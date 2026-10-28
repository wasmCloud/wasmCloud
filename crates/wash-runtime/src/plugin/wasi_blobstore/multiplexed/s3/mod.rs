//! S3 [`BlobBackend`] for the multiplexed blobstore plugin, on `object_store`.
//!
//! Selected with `backend: s3`. Every other key is one of
//! [`CONFIG_KEYS`](config::CONFIG_KEYS), under the names AWS uses, and all of
//! them are host-owned: each decides where a binding connects or as whom. On
//! `wash host` (`workloadConfig: deny`) an operator declares the binding, with
//! credentials from a secret source, and a workload asks for it by name:
//!
//! ```yaml
//! # host config
//! secrets:
//!   s3-assets:
//!     fromEnv: [AWS_ACCESS_KEY_ID, AWS_SECRET_ACCESS_KEY]
//! host:
//!   plugins:
//!     - id: wasi-blobstore-multiplexed
//!       bindings:
//!         assets:
//!           config:
//!             backend: s3
//!             region: eu-central-1
//!           secretFrom: [s3-assets]
//!
//! # workload manifest
//! host_interfaces:
//!   - namespace: wasi
//!     package: blobstore
//!     interfaces: [blobstore, container]
//!     name: assets
//! ```
//!
//! A workload that writes any of these keys itself is refused. On `wash dev`
//! (`allow`) they may sit in the manifest's own `config` instead.
//!
//! Every binding names exactly one credential source: static keys,
//! `skip_signature: true` for anonymous access, or `use_host_identity: true`
//! to run as the host (its `AWS_*` environment, then the instance role). One
//! that names none fails to bind rather than inheriting the host's identity.
//!
//! Under a declared egress policy (`allowedHosts`), each bind checks the
//! endpoint the client dials: `endpoint` when set, otherwise
//! `https://s3.{region}.amazonaws.com`. Virtual-hosted requests without an
//! `endpoint` are refused there, since their host carries the bucket a guest
//! names later. Not checked: the credential endpoints `use_host_identity`
//! reaches (instance metadata, STS), which come from the host's environment,
//! and redirects the HTTP client follows.
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

use std::collections::HashMap;
use std::sync::Arc;
use std::time::Duration;

use anyhow::Context;
use futures::{StreamExt, TryStreamExt};
use object_store::aws::{AmazonS3, AmazonS3Builder, AmazonS3ConfigKey};
use object_store::path::Path;
use object_store::{ObjectStore, ObjectStoreExt};
use sha2::{Digest, Sha256};

use crate::host::declared_port::Protocol;
use crate::plugin::PluginEgressPolicy;
use crate::plugin::multiplex::{BACKEND_CONFIG_KEY, BackendProvider};

use super::{
    BlobBackend, BlobBackendError, BlobId, BlobResult, ContainerInfo, ObjectInfo, clamp_range,
};

mod config;

pub(crate) use config::host_owned_spellings;
use config::{builder_from_config, lookup, normalize_key};

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

    async fn instantiate(
        &self,
        config: &HashMap<String, String>,
        _egress: Option<&PluginEgressPolicy>,
    ) -> anyhow::Result<BlobId> {
        let base = builder_from_config(config)?;
        // Probe-build so a config error fails the bind, not the first request.
        base.clone()
            .with_bucket_name("probe")
            .build()
            .context("invalid s3 blobstore config")?;
        Ok(Arc::new(S3BlobBackend::new(base, MAX_BUCKET_CLIENTS)))
    }

    /// The endpoint the client dials, derived as `AmazonS3Builder::build`
    /// derives it, without the bucket.
    fn check_egress(
        &self,
        config: &HashMap<String, String>,
        policy: &PluginEgressPolicy,
    ) -> anyhow::Result<()> {
        // A config the builder refuses fails in `instantiate`, as the config
        // error it is rather than an egress refusal.
        let Ok(builder) = builder_from_config(config) else {
            return Ok(());
        };
        let get = |key| builder.get_config_value(&key);
        let endpoint =
            get(AmazonS3ConfigKey::S3Endpoint).or_else(|| get(AmazonS3ConfigKey::Endpoint));
        let virtual_hosted =
            get(AmazonS3ConfigKey::VirtualHostedStyleRequest).as_deref() == Some("true");
        let url = match (endpoint, virtual_hosted) {
            (Some(endpoint), _) => endpoint,
            (None, false) => {
                let region = get(AmazonS3ConfigKey::Region).unwrap_or_else(|| "us-east-1".into());
                format!("https://s3.{region}.amazonaws.com")
            }
            (None, true) => anyhow::bail!(
                "s3 blobstore virtual-hosted requests without an 'endpoint' dial a host per \
                 bucket, which allowedHosts cannot check; name an 'endpoint' or use path-style \
                 requests"
            ),
        };
        policy.check_url(&url, Protocol::Tcp, 443)
    }
}

#[cfg(test)]
mod tests {
    use super::config::test_support::{cfg, keyed};
    use super::*;

    fn key(pairs: &[(&str, &str)]) -> String {
        S3BlobProvider.pool_key(&cfg(pairs)).unwrap()
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

    #[tokio::test]
    async fn instantiate_fails_bind_on_bad_config() {
        // `allow_http` is parsed leniently at `with_config`; a deferred bool is
        // what the probe-build catches.
        let Err(err) = S3BlobProvider
            .instantiate(&keyed(&[("virtual_hosted_style_request", "maybe")]), None)
            .await
        else {
            panic!("a bad bool must fail the bind");
        };
        let err = err.to_string();
        assert!(err.contains("invalid s3 blobstore config"), "{err}");
    }

    fn egress(config: &HashMap<String, String>, policy: &PluginEgressPolicy) -> anyhow::Result<()> {
        S3BlobProvider.check_egress(config, policy)
    }

    #[test]
    fn egress_is_checked_at_the_endpoint_s3_dials() {
        let policy = PluginEgressPolicy::allowing(&[
            "https://s3.eu-central-1.amazonaws.com",
            "http://rustfs.internal:9000",
        ]);
        // No endpoint: path-style requests go to the regional endpoint.
        egress(&keyed(&[("region", "eu-central-1")]), &policy).unwrap();
        egress(&keyed(&[("aws_region", "eu-central-1")]), &policy).unwrap();
        assert!(egress(&keyed(&[("region", "us-west-2")]), &policy).is_err());
        // An endpoint, under any spelling, is dialed instead of the region's.
        egress(
            &keyed(&[("endpoint", "http://rustfs.internal:9000")]),
            &policy,
        )
        .unwrap();
        egress(
            &keyed(&[("aws_endpoint_url", "http://rustfs.internal:9000")]),
            &policy,
        )
        .unwrap();
        let err = egress(
            &keyed(&[
                ("region", "eu-central-1"),
                ("endpoint", "https://evil.example"),
            ]),
            &policy,
        )
        .unwrap_err();
        assert!(format!("{err:#}").contains("evil.example"), "{err:#}");
    }

    #[test]
    fn egress_refuses_virtual_hosted_requests_without_an_endpoint() {
        // The host would be `{bucket}.s3.…`, and the guest names the bucket
        // after the bind.
        let policy = PluginEgressPolicy::allowing(&["https://s3.eu-central-1.amazonaws.com"]);
        let err = egress(
            &keyed(&[
                ("region", "eu-central-1"),
                ("virtual_hosted_style_request", "true"),
            ]),
            &policy,
        )
        .unwrap_err();
        assert!(format!("{err:#}").contains("'endpoint'"), "{err:#}");

        // An endpoint for virtual-hosted requests already names the bucket.
        let policy =
            PluginEgressPolicy::allowing(&["https://assets.s3.eu-central-1.amazonaws.com"]);
        egress(
            &keyed(&[
                ("virtual_hosted_style_request", "true"),
                ("endpoint", "https://assets.s3.eu-central-1.amazonaws.com"),
            ]),
            &policy,
        )
        .unwrap();
    }

    #[tokio::test]
    async fn egress_leaves_a_bad_config_to_instantiate() {
        // No credential source: refused by the bind as a config error, not
        // reported as an egress refusal.
        let policy = PluginEgressPolicy::allowing(&[]);
        let config = cfg(&[("region", "eu-central-1")]);
        egress(&config, &policy).unwrap();
        assert!(
            S3BlobProvider
                .instantiate(&config, Some(&policy))
                .await
                .is_err()
        );
    }

    #[test]
    fn egress_refusal_does_not_echo_credentials() {
        let policy = PluginEgressPolicy::allowing(&["https://s3.eu-central-1.amazonaws.com"]);
        let err = egress(
            &cfg(&[
                ("region", "us-west-2"),
                ("access_key_id", "AKIA-OPERATOR"),
                ("secret_access_key", "operator-secret"),
            ]),
            &policy,
        )
        .unwrap_err();
        let err = format!("{err:#}");
        assert!(!err.contains("AKIA-OPERATOR"), "{err}");
        assert!(!err.contains("operator-secret"), "{err}");
    }
}
