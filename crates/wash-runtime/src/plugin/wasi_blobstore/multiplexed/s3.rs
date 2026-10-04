//! S3 [`BlobBackend`] for the multiplexed blobstore plugin, on `object_store`.
//!
//! Selected per host interface with `backend: s3`; the other keys are the
//! ones in [`CONFIG_KEYS`], under the names AWS uses:
//!
//! ```yaml
//! host_interfaces:
//!   - namespace: wasi
//!     package: blobstore
//!     interfaces: [blobstore, container]
//!     config:
//!       backend: s3
//!       endpoint: https://s3.eu-central-1.amazonaws.com
//!       region: eu-central-1
//!       access_key_id: AKIA...
//!       secret_access_key: ...
//! ```
//!
//! Every binding names exactly one credential source: static keys, `skip_signature:
//! true` for anonymous access, or `use_host_identity: true` to run as the
//! host (its `AWS_*` environment, then the instance role). The last one is a
//! host-owned key, so under `workloadConfig: deny` only an operator's
//! `host.plugins` binding can grant it; a workload that names no source fails
//! to bind rather than inheriting the host's identity.
//!
//! Each container is one bucket, provisioned outside wasmCloud:
//! `create_container` and `delete_container` return an error until
//! `object_store` grows a bucket API (apache/arrow-rs-object-store#868).
//!
//! Where S3 or `object_store` show through the `wasi:blobstore` surface:
//!
//! - `container_exists` on a missing bucket is an error, not `false`, since
//!   `object_store` reports a list 404 and 403 alike
//!   (apache/arrow-rs-object-store#851). `get_container` costs one LIST.
//! - A 404 on a read is `NoSuchObject` even when it is the bucket that is
//!   missing; only writes and deletes can tell the two apart.
//! - `copy_object` and `move_object` across buckets go through host memory;
//!   `object_store` only copies within one bucket.

use std::collections::{BTreeMap, HashMap};
use std::sync::Arc;
use std::time::Duration;

use anyhow::Context;
use futures::{StreamExt, TryStreamExt};
use object_store::ClientConfigKey;
use object_store::aws::{AmazonS3, AmazonS3Builder, AmazonS3ConfigKey};
use object_store::path::Path;
use object_store::{ObjectStore, ObjectStoreExt};
use sha2::{Digest, Sha256};

use crate::plugin::multiplex::{BACKEND_CONFIG_KEY, BackendProvider};

use super::{
    BlobBackend, BlobBackendError, BlobId, BlobResult, ContainerInfo, ObjectInfo, clamp_range,
};

/// Opts a binding into the host's own AWS identity (`AWS_*` env, then IMDS).
/// Host-owned: see the plugins' `binding_schema`.
pub(crate) const USE_HOST_IDENTITY_KEY: &str = "use_host_identity";

/// Ceiling on bucket clients kept per backend, and how long an unused one
/// stays. Each client owns an HTTP connection pool, and a guest can name as
/// many buckets as it likes, so the cache must be bounded. The numbers match
/// the multiplexer's pooled-connection defaults.
const MAX_BUCKET_CLIENTS: u64 = 64;
const BUCKET_CLIENT_IDLE: Duration = Duration::from_secs(300);

/// An S3 [`BlobBackend`]: each container is one bucket, which must already exist.
pub struct S3BlobBackend {
    base: AmazonS3Builder,
    stores: moka::sync::Cache<String, Arc<AmazonS3>>,
}

impl S3BlobBackend {
    fn new(base: AmazonS3Builder, max_clients: u64) -> Self {
        Self {
            base,
            stores: moka::sync::Cache::builder()
                .max_capacity(max_clients)
                .time_to_idle(BUCKET_CLIENT_IDLE)
                .build(),
        }
    }

    /// The bucket-scoped client for `container`, built on first use. Does not
    /// check that the bucket exists; a missing one is a 404 on first use.
    fn store(&self, container: &str) -> BlobResult<Arc<AmazonS3>> {
        self.stores
            .try_get_with_by_ref(container, || {
                self.base
                    .clone()
                    .with_bucket_name(container)
                    .build()
                    .map(Arc::new)
                    .map_err(|e| {
                        BlobBackendError::other(format!(
                            "failed to build s3 client for container '{container}': {e}"
                        ))
                    })
            })
            .map_err(|e| (*e).clone())
    }

    /// `Path::parse` refuses `..`, empty segments and control characters instead
    /// of silently rewriting them like `Path::from` would.
    fn object_path(object: &str) -> BlobResult<Path> {
        Path::parse(object)
            .map_err(|e| BlobBackendError::other(format!("invalid object name '{object}': {e}")))
    }

    /// Errors from an operation addressed at an object: a 404 is the object.
    fn object_err(e: object_store::Error, object: &str) -> BlobBackendError {
        match e {
            object_store::Error::NotFound { .. } => {
                BlobBackendError::NoSuchObject(object.to_string())
            }
            _ => BlobBackendError::other(format!("s3 error: {e}")),
        }
    }

    /// Errors from an operation addressed at the bucket (put, delete, list): S3
    /// answers 204 for a missing key, so a 404 here is the bucket.
    fn container_err(e: object_store::Error, container: &str) -> BlobBackendError {
        match e {
            object_store::Error::NotFound { .. } => {
                BlobBackendError::NoSuchContainer(container.to_string())
            }
            _ => BlobBackendError::other(format!("s3 error: {e}")),
        }
    }
}

#[async_trait::async_trait]
impl BlobBackend for S3BlobBackend {
    async fn create_container(&self, name: &str) -> BlobResult<()> {
        match self.container_exists(name).await {
            Ok(true) => Err(BlobBackendError::ContainerAlreadyExists(name.to_string())),
            Ok(false) => Err(BlobBackendError::other(format!(
                "s3 blobstore cannot create buckets; create '{name}' outside wasmCloud"
            ))),
            Err(e) => Err(BlobBackendError::other(format!(
                "s3 blobstore cannot create buckets; create '{name}' outside wasmCloud ({e})"
            ))),
        }
    }

    async fn get_container(&self, name: &str) -> BlobResult<()> {
        if self.container_exists(name).await? {
            Ok(())
        } else {
            Err(BlobBackendError::NoSuchContainer(name.to_string()))
        }
    }

    async fn delete_container(&self, name: &str) -> BlobResult<()> {
        Err(BlobBackendError::other(format!(
            "s3 blobstore cannot delete buckets; delete '{name}' outside wasmCloud"
        )))
    }

    async fn container_exists(&self, name: &str) -> BlobResult<bool> {
        let store = self.store(name)?;
        match store.list(None).next().await {
            None | Some(Ok(_)) => Ok(true),
            // Unreachable while object_store#851 turns a list 404 into
            // `Generic`; kept so the fix lands without a code change here.
            Some(Err(object_store::Error::NotFound { .. })) => Ok(false),
            Some(Err(e)) => Err(Self::container_err(e, name)),
        }
    }

    async fn container_info(&self, name: &str) -> BlobResult<ContainerInfo> {
        self.get_container(name).await?;
        Ok(ContainerInfo {
            name: name.to_string(),
            created_at: 0,
        })
    }

    async fn clear_container(&self, name: &str) -> BlobResult<()> {
        let store = self.store(name)?;
        let locations = store.list(None).map_ok(|meta| meta.location).boxed();
        store
            .delete_stream(locations)
            .try_for_each(|_| futures::future::ok(()))
            .await
            .map_err(|e| Self::container_err(e, name))
    }

    async fn get_data(
        &self,
        container: &str,
        object: &str,
        start: u64,
        end: u64,
    ) -> BlobResult<Vec<u8>> {
        let store = self.store(container)?;
        let path = Self::object_path(object)?;
        let meta = store
            .head(&path)
            .await
            .map_err(|e| Self::object_err(e, object))?;
        let len = usize::try_from(meta.size).unwrap_or(usize::MAX);
        let range = clamp_range(start, end, len);
        if range.is_empty() {
            return Ok(Vec::new());
        }
        let bytes = store
            .get_range(&path, range.start as u64..range.end as u64)
            .await
            .map_err(|e| Self::object_err(e, object))?;
        Ok(bytes.into())
    }

    async fn write_data(&self, container: &str, object: &str, data: Vec<u8>) -> BlobResult<()> {
        let store = self.store(container)?;
        let path = Self::object_path(object)?;
        store
            .put(&path, data.into())
            .await
            .map_err(|e| Self::container_err(e, container))?;
        Ok(())
    }

    async fn list_objects(&self, container: &str) -> BlobResult<Vec<String>> {
        let store = self.store(container)?;
        store
            .list(None)
            .map_ok(|meta| meta.location.to_string())
            .try_collect::<Vec<_>>()
            .await
            .map_err(|e| Self::container_err(e, container))
    }

    async fn delete_object(&self, container: &str, object: &str) -> BlobResult<()> {
        let store = self.store(container)?;
        let path = Self::object_path(object)?;
        store
            .delete(&path)
            .await
            .map_err(|e| Self::container_err(e, container))?;
        Ok(())
    }

    async fn delete_objects(&self, container: &str, objects: &[String]) -> BlobResult<()> {
        if objects.is_empty() {
            return Ok(());
        }
        let store = self.store(container)?;
        let paths = objects
            .iter()
            .map(|o| Self::object_path(o))
            .collect::<BlobResult<Vec<_>>>()?;
        store
            .delete_stream(futures::stream::iter(paths.into_iter().map(Ok)).boxed())
            .try_for_each(|_| futures::future::ok(()))
            .await
            .map_err(|e| Self::container_err(e, container))
    }

    async fn has_object(&self, container: &str, object: &str) -> BlobResult<bool> {
        let store = self.store(container)?;
        let path = Self::object_path(object)?;
        match store.head(&path).await {
            Ok(_) => Ok(true),
            Err(object_store::Error::NotFound { .. }) => Ok(false),
            Err(e) => Err(Self::object_err(e, object)),
        }
    }

    async fn object_info(&self, container: &str, object: &str) -> BlobResult<ObjectInfo> {
        let store = self.store(container)?;
        let path = Self::object_path(object)?;
        let meta = store
            .head(&path)
            .await
            .map_err(|e| Self::object_err(e, object))?;
        Ok(ObjectInfo {
            name: object.to_string(),
            container: container.to_string(),
            // S3 keeps no creation time; last-modified is the closest it has.
            created_at: u64::try_from(meta.last_modified.timestamp()).unwrap_or(0),
            size: meta.size,
        })
    }

    async fn copy_object(
        &self,
        src_container: &str,
        src_object: &str,
        dest_container: &str,
        dest_object: &str,
    ) -> BlobResult<()> {
        if src_container == dest_container {
            let store = self.store(src_container)?;
            store
                .copy(
                    &Self::object_path(src_object)?,
                    &Self::object_path(dest_object)?,
                )
                .await
                .map_err(|e| Self::object_err(e, src_object))?;
        } else {
            let data = self
                .get_data(src_container, src_object, 0, u64::MAX)
                .await?;
            self.write_data(dest_container, dest_object, data).await?;
        }
        Ok(())
    }
}

/// One manifest config key the s3 backend forwards to `object_store`.
struct ConfigKeySpec {
    /// Canonical name, used in `pool_key` and in error messages.
    name: &'static str,
    /// Every spelling accepted, already lower-case with `_` (callers normalize first).
    aliases: &'static [&'static str],
    key: AmazonS3ConfigKey,
}

/// Forwarded keys, under every spelling `AmazonS3ConfigKey` accepts. Not here,
/// and refused: `bucket` (the container is the bucket), and the keys that read
/// host files or dial manifest-chosen URLs (`web_identity_token_file`,
/// `container_*`, `metadata_endpoint`, `role_arn`, proxy and certificate keys).
const CONFIG_KEYS: &[ConfigKeySpec] = &[
    ConfigKeySpec {
        name: "region",
        aliases: &["region", "aws_region"],
        key: AmazonS3ConfigKey::Region,
    },
    ConfigKeySpec {
        name: "default_region",
        aliases: &["default_region", "aws_default_region"],
        key: AmazonS3ConfigKey::DefaultRegion,
    },
    ConfigKeySpec {
        name: "endpoint",
        aliases: &[
            "endpoint",
            "aws_endpoint",
            "endpoint_url",
            "aws_endpoint_url",
        ],
        key: AmazonS3ConfigKey::Endpoint,
    },
    ConfigKeySpec {
        name: "access_key_id",
        aliases: &["access_key_id", "aws_access_key_id"],
        key: AmazonS3ConfigKey::AccessKeyId,
    },
    ConfigKeySpec {
        name: "secret_access_key",
        aliases: &["secret_access_key", "aws_secret_access_key"],
        key: AmazonS3ConfigKey::SecretAccessKey,
    },
    ConfigKeySpec {
        name: "session_token",
        aliases: &["session_token", "aws_session_token", "aws_token", "token"],
        key: AmazonS3ConfigKey::Token,
    },
    ConfigKeySpec {
        name: "allow_http",
        aliases: &["allow_http", "aws_allow_http"],
        key: AmazonS3ConfigKey::Client(ClientConfigKey::AllowHttp),
    },
    ConfigKeySpec {
        name: "virtual_hosted_style_request",
        aliases: &[
            "virtual_hosted_style_request",
            "aws_virtual_hosted_style_request",
        ],
        key: AmazonS3ConfigKey::VirtualHostedStyleRequest,
    },
    ConfigKeySpec {
        name: "skip_signature",
        aliases: &["skip_signature", "aws_skip_signature"],
        key: AmazonS3ConfigKey::SkipSignature,
    },
];

/// Trim, lower-case, `-` → `_`. The host folds `_`/`-`/case for ownership
/// checks (`bindings::canonical_key`) but hands the plugin the written spelling.
fn normalize_key(key: &str) -> String {
    key.trim().to_ascii_lowercase().replace('-', "_")
}

fn lookup(normalized: &str) -> Option<&'static ConfigKeySpec> {
    CONFIG_KEYS.iter().find(|k| k.aliases.contains(&normalized))
}

/// One manifest setting after folding its spellings onto a canonical name.
struct Folded<'a> {
    /// The spelling the manifest used, for the conflict message.
    written: &'a str,
    value: &'a str,
    /// The `object_store` key it is written to; `None` for a key this backend
    /// reads itself.
    target: Option<AmazonS3ConfigKey>,
}

/// The bucket-less base builder from a binding's config. Errors name keys,
/// never values (#5470).
fn builder_from_config(config: &HashMap<String, String>) -> anyhow::Result<AmazonS3Builder> {
    // Fold every spelling onto its canonical name first: two spellings of
    // one key with different values would otherwise be applied in `HashMap`
    // order, and the host only folds spellings for keys a schema declares.
    let mut folded: BTreeMap<&'static str, Folded<'_>> = BTreeMap::new();
    for (key, value) in config {
        let normalized = normalize_key(key);
        if normalized == BACKEND_CONFIG_KEY {
            continue;
        }
        let (name, target) = if normalized == USE_HOST_IDENTITY_KEY {
            (USE_HOST_IDENTITY_KEY, None)
        } else {
            let Some(spec) = lookup(&normalized) else {
                anyhow::bail!(
                    "unknown s3 blobstore config key '{key}'; accepted keys: {}",
                    CONFIG_KEYS
                        .iter()
                        .map(|k| k.name)
                        .collect::<Vec<_>>()
                        .join(", ")
                );
            };
            // `build()` prefers `S3Endpoint` (`AWS_ENDPOINT_URL_S3`) over
            // `Endpoint`, so a manifest `endpoint` written to the latter would
            // lose to the host's environment. Write the one that wins.
            let target = if spec.key == AmazonS3ConfigKey::Endpoint {
                AmazonS3ConfigKey::S3Endpoint
            } else {
                spec.key
            };
            (spec.name, Some(target))
        };
        if let Some(first) = folded.get(name) {
            if first.value != value {
                anyhow::bail!(
                    "conflicting values for '{name}' (written '{}' and '{key}', which are one key)",
                    first.written
                );
            }
            continue;
        }
        folded.insert(
            name,
            Folded {
                written: key,
                value,
                target,
            },
        );
    }

    // Whether this binding may use the host's own AWS identity at all.
    let use_host_identity = folded
        .remove(USE_HOST_IDENTITY_KEY)
        .map(|f| f.value.parse::<bool>())
        .transpose()
        .map_err(|_| {
            anyhow::anyhow!("invalid value for '{USE_HOST_IDENTITY_KEY}'; expected true or false")
        })?
        .unwrap_or(false);

    // Every binding states where its credentials come from. Without this, a
    // binding that names none would fall through to the host's environment
    // and instance role: object_store's default chain ends at IMDS.
    let has_access_key = folded.contains_key("access_key_id");
    let has_secret_key = folded.contains_key("secret_access_key");
    if has_access_key != has_secret_key {
        anyhow::bail!(
            "s3 blobstore binding sets one of access_key_id and secret_access_key; static \
             credentials need both"
        );
    }
    let has_static_keys = has_access_key && has_secret_key;
    let anonymous = folded
        .get("skip_signature")
        .is_some_and(|f| f.value == "true");
    let sources = [has_static_keys, anonymous, use_host_identity]
        .into_iter()
        .filter(|set| *set)
        .count();
    if sources != 1 {
        anyhow::bail!(
            "s3 blobstore binding must name exactly one credential source (found {sources}): \
             access_key_id and secret_access_key, or skip_signature: true, or \
             {USE_HOST_IDENTITY_KEY}: true (host-owned under workloadConfig: deny)"
        );
    }

    let mut builder = if use_host_identity {
        AmazonS3Builder::from_env()
    } else {
        AmazonS3Builder::new()
    };
    for setting in folded.into_values() {
        if let Some(target) = setting.target {
            builder = builder.with_config(target, setting.value);
        }
    }
    Ok(builder)
}

/// Provider for [`S3BlobBackend`], selected by `config.backend = "s3"`.
#[derive(Default)]
pub struct S3BlobProvider;

#[async_trait::async_trait]
impl BackendProvider<BlobId> for S3BlobProvider {
    fn backend_type(&self) -> &'static str {
        "s3"
    }

    /// Hash of the whole config, length-prefixed: no plaintext secret in the
    /// pool map. Secrets and session tokens are *in* it on purpose, since a
    /// pool hit refreshes `last_used` and would otherwise pin a stale token.
    fn pool_key(&self, config: &HashMap<String, String>) -> Option<String> {
        let mut pairs: Vec<(String, &str)> = config
            .iter()
            .filter_map(|(key, value)| {
                let normalized = normalize_key(key);
                if normalized == BACKEND_CONFIG_KEY {
                    return None;
                }
                let name = lookup(&normalized)
                    .map(|spec| spec.name.to_string())
                    .unwrap_or(normalized);
                Some((name, value.as_str()))
            })
            .collect();
        pairs.sort();
        let mut hasher = Sha256::new();
        for (name, value) in pairs {
            hasher.update((name.len() as u64).to_le_bytes());
            hasher.update(name.as_bytes());
            hasher.update((value.len() as u64).to_le_bytes());
            hasher.update(value.as_bytes());
        }
        Some(format!("{:x}", hasher.finalize()))
    }

    async fn instantiate(&self, config: &HashMap<String, String>) -> anyhow::Result<BlobId> {
        let base = builder_from_config(config)?;
        // Probe-build so a config error fails the bind, not the first request.
        base.clone()
            .with_bucket_name("probe")
            .build()
            .context("invalid s3 blobstore config")?;
        Ok(Arc::new(S3BlobBackend::new(base, MAX_BUCKET_CLIENTS)))
    }
}

#[cfg(test)]
mod tests {
    use super::*;

    fn cfg(pairs: &[(&str, &str)]) -> HashMap<String, String> {
        pairs
            .iter()
            .map(|(k, v)| (k.to_string(), v.to_string()))
            .collect()
    }

    fn key(pairs: &[(&str, &str)]) -> String {
        S3BlobProvider.pool_key(&cfg(pairs)).unwrap()
    }

    /// `pairs` plus static keys, for tests about something other than where
    /// credentials come from.
    fn keyed(pairs: &[(&str, &str)]) -> HashMap<String, String> {
        let mut config = cfg(&[("access_key_id", "AKIA"), ("secret_access_key", "s")]);
        config.extend(cfg(pairs));
        config
    }

    #[test]
    fn config_keys_match_object_store_spellings() {
        for spec in CONFIG_KEYS {
            assert!(
                spec.aliases.contains(&spec.name),
                "canonical name '{}' must be one of its own spellings",
                spec.name
            );
            for alias in spec.aliases {
                let parsed: AmazonS3ConfigKey = alias
                    .parse()
                    .unwrap_or_else(|e| panic!("object_store rejects '{alias}': {e}"));
                assert_eq!(parsed, spec.key, "'{alias}' resolves to the wrong key");
            }
        }
    }

    #[test]
    fn pool_key_folds_spelling_not_values() {
        let a = key(&[("aws_region", "us-east-1")]);
        assert_eq!(a, key(&[("Region", "us-east-1")]));
        assert_eq!(a, key(&[(" aws-region ", "us-east-1")]));
        assert_ne!(a, key(&[("region", "us-west-2")]));
    }

    #[test]
    fn pool_key_ignores_backend() {
        assert_eq!(
            key(&[("region", "x")]),
            key(&[("backend", "s3"), ("region", "x")])
        );
    }

    #[test]
    fn pool_key_is_injective_over_nul() {
        // A naive `k=v\0k=v` join would make these two configs collide.
        assert_ne!(
            key(&[("endpoint", "a\u{0}region=b")]),
            key(&[("endpoint", "a"), ("region", "b")])
        );
    }

    #[test]
    fn pool_key_has_no_plaintext_secret() {
        let k = key(&[("secret_access_key", "hunter2-not-in-key")]);
        assert_eq!(k.len(), 64, "sha-256 hex");
        assert!(k.chars().all(|c| c.is_ascii_hexdigit()));
        assert!(!k.contains("hunter2"));
    }

    #[test]
    fn pool_key_changes_with_session_token() {
        // A rotated token must land on a fresh backend, not a pooled stale one.
        assert_ne!(
            key(&[("access_key_id", "AKIA"), ("session_token", "t1")]),
            key(&[("access_key_id", "AKIA"), ("session_token", "t2")])
        );
    }

    #[test]
    fn pool_key_is_some_for_empty_config() {
        assert!(S3BlobProvider.pool_key(&HashMap::new()).is_some());
    }

    #[test]
    fn builder_rejects_unknown_key_naming_it_only() {
        let err = builder_from_config(&cfg(&[("buckets", "VALUE-MUST-NOT-LEAK")]))
            .expect_err("unknown key must be refused")
            .to_string();
        assert!(err.contains("'buckets'"), "names the key: {err}");
        assert!(
            err.contains("accepted keys:"),
            "lists what is accepted: {err}"
        );
        assert!(err.contains("region"), "{err}");
        assert!(
            !err.contains("VALUE-MUST-NOT-LEAK"),
            "must not echo the value: {err}"
        );
    }

    #[test]
    fn builder_rejects_bucket() {
        // The container is the bucket; a fixed one would silently win or lose.
        for spelling in ["bucket", "bucket_name", "aws_bucket", "aws_bucket_name"] {
            assert!(
                builder_from_config(&cfg(&[(spelling, "x")])).is_err(),
                "'{spelling}' must be refused"
            );
        }
    }

    #[test]
    fn builder_rejects_conflicting_spellings_naming_both() {
        // Two spellings of one key with different values would otherwise be
        // applied in `HashMap` order, so the winner would change per bind.
        let err = builder_from_config(&cfg(&[
            ("endpoint", "http://ONE-MUST-NOT-LEAK"),
            ("aws_endpoint", "http://TWO-MUST-NOT-LEAK"),
        ]))
        .expect_err("conflicting spellings must be refused")
        .to_string();
        assert!(err.contains("conflicting values for 'endpoint'"), "{err}");
        assert!(
            err.contains("'endpoint'") && err.contains("'aws_endpoint'"),
            "names both spellings: {err}"
        );
        assert!(
            !err.contains("MUST-NOT-LEAK"),
            "must not echo the values: {err}"
        );
    }

    #[test]
    fn builder_accepts_agreeing_spellings() {
        let builder = builder_from_config(&keyed(&[
            ("endpoint", "http://same:9000"),
            ("aws_endpoint", "http://same:9000"),
        ]))
        .unwrap();
        assert_eq!(
            builder
                .get_config_value(&AmazonS3ConfigKey::S3Endpoint)
                .as_deref(),
            Some("http://same:9000")
        );
    }

    #[test]
    fn builder_accepts_every_spelling() {
        for spec in CONFIG_KEYS {
            // A credential source that does not collide with the key under test.
            let source: &[(&str, &str)] = match spec.name {
                "access_key_id" => &[("secret_access_key", "s")],
                "secret_access_key" => &[("access_key_id", "AKIA")],
                _ => &[("access_key_id", "AKIA"), ("secret_access_key", "s")],
            };
            for alias in spec.aliases {
                let mut config = cfg(source);
                config.extend(cfg(&[(alias, "v")]));
                assert!(
                    builder_from_config(&config).is_ok(),
                    "'{alias}' must be accepted"
                );
            }
        }
    }

    #[test]
    fn bucket_clients_are_bounded() {
        // A guest can name any number of buckets; the per-bucket clients it
        // leaves behind must not grow without limit. `build()` is local, so
        // this needs no server.
        let base = builder_from_config(&keyed(&[])).unwrap();
        let backend = S3BlobBackend::new(base, 2);
        for name in ["probe-1", "probe-2", "probe-3", "probe-4"] {
            backend.store(name).unwrap();
        }
        backend.stores.run_pending_tasks();
        assert!(
            backend.stores.entry_count() <= 2,
            "cache holds {} clients, expected at most 2",
            backend.stores.entry_count()
        );
        // A hit returns the same client rather than rebuilding it.
        let a = backend.store("hit").unwrap();
        let b = backend.store("hit").unwrap();
        assert!(Arc::ptr_eq(&a, &b));
    }

    #[test]
    fn builder_requires_a_credential_source() {
        // A binding that names none must not fall through to the host's
        // environment and instance role.
        let err = builder_from_config(&cfg(&[("region", "us-east-1")]))
            .expect_err("no credential source must be refused")
            .to_string();
        assert!(err.contains("credential source"), "{err}");
        assert!(
            err.contains(USE_HOST_IDENTITY_KEY),
            "points at the opt-in: {err}"
        );
    }

    #[test]
    fn builder_accepts_anonymous_access() {
        assert!(builder_from_config(&cfg(&[("skip_signature", "true")])).is_ok());
        // Anything but a literal `true` is not anonymous; object_store rejects
        // the value later, at the probe build.
        assert!(builder_from_config(&cfg(&[("skip_signature", "yes")])).is_err());
    }

    #[test]
    fn builder_uses_host_identity_only_when_asked() {
        assert!(builder_from_config(&cfg(&[(USE_HOST_IDENTITY_KEY, "true")])).is_ok());
        assert!(builder_from_config(&cfg(&[("Use-Host-Identity", "true")])).is_ok());
        let err = builder_from_config(&cfg(&[(USE_HOST_IDENTITY_KEY, "MAYBE-MUST-NOT-LEAK")]))
            .expect_err("a non-bool must be refused")
            .to_string();
        assert!(err.contains(USE_HOST_IDENTITY_KEY), "{err}");
        assert!(
            !err.contains("MUST-NOT-LEAK"),
            "must not echo the value: {err}"
        );
        // Exactly one source: any two of static keys, anonymous and the host
        // identity are refused, naming the count.
        for two in [
            keyed(&[(USE_HOST_IDENTITY_KEY, "true")]),
            keyed(&[("skip_signature", "true")]),
            cfg(&[("skip_signature", "true"), (USE_HOST_IDENTITY_KEY, "true")]),
        ] {
            let err = builder_from_config(&two)
                .expect_err("two credential sources must be refused")
                .to_string();
            assert!(
                err.contains("exactly one credential source (found 2)"),
                "{err}"
            );
        }
        // `skip_signature: false` is not a source, so it may sit next to keys.
        assert!(builder_from_config(&keyed(&[("skip_signature", "false")])).is_ok());
        // Spellings fold like any other key: agreeing ones are one setting,
        // disagreeing ones are refused rather than resolved in map order.
        assert!(
            builder_from_config(&cfg(&[
                (USE_HOST_IDENTITY_KEY, "true"),
                ("Use-Host-Identity", "true"),
            ]))
            .is_ok()
        );
        let err = builder_from_config(&cfg(&[
            (USE_HOST_IDENTITY_KEY, "true"),
            ("use-host-identity", "false"),
        ]))
        .expect_err("disagreeing spellings must be refused")
        .to_string();
        assert!(
            err.contains("conflicting values for 'use_host_identity'"),
            "{err}"
        );
        assert!(
            err.contains("'use_host_identity'") && err.contains("'use-host-identity'"),
            "names both spellings: {err}"
        );
    }

    #[test]
    fn builder_requires_both_static_keys() {
        // object_store refuses a lone key at `build()` too, but the rule is
        // ours to state: half a credential is not a credential source.
        for lone in ["access_key_id", "secret_access_key"] {
            let err = builder_from_config(&cfg(&[(lone, "x")]))
                .expect_err("a lone key must be refused")
                .to_string();
            assert!(
                err.contains("access_key_id") && err.contains("secret_access_key"),
                "'{lone}' alone: {err}"
            );
        }
    }

    #[test]
    fn builder_writes_endpoint_where_build_reads_it() {
        // `S3Endpoint` beats `Endpoint` in `build()`; the manifest must land
        // on the winning side or `AWS_ENDPOINT_URL_S3` in the host env wins.
        let builder = builder_from_config(&keyed(&[("endpoint", "http://manifest:9000")])).unwrap();
        assert_eq!(
            builder
                .get_config_value(&AmazonS3ConfigKey::S3Endpoint)
                .as_deref(),
            Some("http://manifest:9000")
        );
    }

    #[tokio::test]
    async fn instantiate_fails_bind_on_bad_config() {
        // `allow_http` is parsed leniently at `with_config`; a deferred bool is
        // what the probe-build catches.
        let Err(err) = S3BlobProvider
            .instantiate(&keyed(&[("virtual_hosted_style_request", "maybe")]))
            .await
        else {
            panic!("a bad bool must fail the bind");
        };
        let err = err.to_string();
        assert!(err.contains("invalid s3 blobstore config"), "{err}");
    }
}
