//! Cache policy and authentication against a loopback OCI registry.
#![cfg(feature = "oci")]
#![allow(clippy::unwrap_used, clippy::expect_used)]

use std::sync::atomic::Ordering;
use std::time::Duration;

use wash_runtime::oci::{OciConfig, OciPullPolicy, pull_component};

#[path = "common/oci_registry.rs"]
mod registry;
use registry::{Image, Registry};

#[tokio::test]
async fn private_cached_images_do_not_cross_credentials() {
    wash_runtime::init_crypto();
    let registry = Registry::start().await;
    let cache = tempfile::tempdir().unwrap();
    let config = OciConfig {
        credentials: Some(("user".into(), "secret".into())),
        cache_dir: Some(cache.path().to_owned()),
        cache: None,
        insecure: true,
        timeout: Some(Duration::from_secs(5)),
    };
    for reference in [
        registry.reference(),
        format!("{}/private/component:v1", registry.addr),
    ] {
        let (bytes, _) = pull_component(&reference, config.clone(), OciPullPolicy::IfNotPresent)
            .await
            .unwrap();
        assert_eq!(bytes, registry.image.read().unwrap().wasm);
        let fetched = registry.requests.load(Ordering::SeqCst);
        pull_component(&reference, config.clone(), OciPullPolicy::Never)
            .await
            .unwrap();
        assert_eq!(
            registry.requests.load(Ordering::SeqCst),
            fetched,
            "Never is offline"
        );

        for credentials in [Some(("user".into(), "wrong".into())), None] {
            let rejected = OciConfig {
                credentials,
                ..config.clone()
            };
            assert!(
                pull_component(&reference, rejected.clone(), OciPullPolicy::Never)
                    .await
                    .is_err()
            );
            assert!(
                pull_component(&reference, rejected, OciPullPolicy::IfNotPresent)
                    .await
                    .is_err()
            );
        }
    }
}

/// A rotated password keeps the user's disk entries: one revalidation with
/// the registry, not a download, makes them usable offline again.
#[tokio::test]
async fn a_password_rotation_revalidates_instead_of_downloading() {
    wash_runtime::init_crypto();
    let registry = Registry::start().await;
    let cache = tempfile::tempdir().unwrap();
    let config = |password: &str| OciConfig {
        credentials: Some(("user".into(), password.into())),
        cache_dir: Some(cache.path().to_owned()),
        cache: None,
        insecure: true,
        timeout: Some(Duration::from_secs(5)),
    };
    let reference = format!("{}/private/component:v1", registry.addr);
    pull_component(&reference, config("secret"), OciPullPolicy::IfNotPresent)
        .await
        .unwrap();
    let download = registry.requests.load(Ordering::SeqCst);

    *registry.authorization.write().unwrap() = "Basic dXNlcjpyb3RhdGVk".into(); // user:rotated
    assert!(
        pull_component(&reference, config("rotated"), OciPullPolicy::Never)
            .await
            .is_err(),
        "Never cannot know the registry accepts the new password"
    );
    let before = registry.requests.load(Ordering::SeqCst);
    let (bytes, _) = pull_component(&reference, config("rotated"), OciPullPolicy::IfNotPresent)
        .await
        .unwrap();
    assert_eq!(bytes, registry.image.read().unwrap().wasm);
    let revalidation = registry.requests.load(Ordering::SeqCst) - before;
    assert!(
        revalidation < download,
        "revalidating took {revalidation} requests, as many as the {download} of a download"
    );

    let before = registry.requests.load(Ordering::SeqCst);
    pull_component(&reference, config("rotated"), OciPullPolicy::Never)
        .await
        .unwrap();
    assert_eq!(registry.requests.load(Ordering::SeqCst), before);
}

#[tokio::test]
async fn always_resolves_repushed_tags_and_retries_completed_failures() {
    wash_runtime::init_crypto();
    let registry = Registry::start().await;
    let config = OciConfig {
        credentials: Some(("user".into(), "secret".into())),
        insecure: true,
        timeout: Some(Duration::from_secs(5)),
        ..Default::default()
    };
    let reference = format!("{}/private/component:latest", registry.addr);
    let first = pull_component(&reference, config.clone(), OciPullPolicy::Always)
        .await
        .unwrap();
    *registry.image.write().unwrap() = Image::new("(component (core module))");
    let second = pull_component(&reference, config.clone(), OciPullPolicy::Always)
        .await
        .unwrap();
    assert_ne!(
        first.1, second.1,
        "a completed Always pull must resolve the tag again"
    );
    assert_ne!(first.0, second.0);

    registry.unavailable.store(true, Ordering::SeqCst);
    assert!(
        pull_component(&reference, config.clone(), OciPullPolicy::Always)
            .await
            .is_err()
    );
    registry.unavailable.store(false, Ordering::SeqCst);
    assert!(
        pull_component(&reference, config, OciPullPolicy::Always)
            .await
            .is_ok()
    );
}

#[tokio::test]
async fn a_plain_http_cache_hit_cannot_satisfy_an_https_pull() {
    wash_runtime::init_crypto();
    let registry = Registry::start().await;
    let cache = tempfile::tempdir().unwrap();
    let config = OciConfig {
        credentials: Some(("user".into(), "secret".into())),
        cache_dir: Some(cache.path().to_owned()),
        cache: None,
        insecure: true,
        timeout: Some(Duration::from_secs(5)),
    };
    let reference = registry.reference();
    pull_component(&reference, config.clone(), OciPullPolicy::IfNotPresent)
        .await
        .unwrap();
    let fetched = registry.requests.load(Ordering::SeqCst);
    let secure = OciConfig {
        insecure: false,
        ..config.clone()
    };
    assert!(
        pull_component(&reference, secure.clone(), OciPullPolicy::Never)
            .await
            .is_err()
    );
    assert_eq!(
        registry.requests.load(Ordering::SeqCst),
        fetched,
        "Never stays offline on a scope miss"
    );
    assert!(
        pull_component(&reference, secure, OciPullPolicy::IfNotPresent)
            .await
            .is_err(),
        "HTTPS cannot reuse bytes fetched over HTTP"
    );
    pull_component(&reference, config, OciPullPolicy::Never)
        .await
        .unwrap();
}

#[tokio::test]
async fn a_pinned_disk_hit_still_validates_the_registry() {
    wash_runtime::init_crypto();
    let registry = Registry::start().await;
    let cache = tempfile::tempdir().unwrap();
    let config = OciConfig {
        credentials: Some(("user".into(), "secret".into())),
        cache_dir: Some(cache.path().to_owned()),
        cache: None,
        insecure: true,
        timeout: Some(Duration::from_secs(5)),
    };
    let reference = registry.reference();
    // Always has a different completed-result key, so the subsequent
    // IfNotPresent request must inspect the persistent entry.
    pull_component(&reference, config.clone(), OciPullPolicy::Always)
        .await
        .unwrap();
    let fetched = registry.requests.load(Ordering::SeqCst);
    registry.unavailable.store(true, Ordering::SeqCst);
    pull_component(&reference, config.clone(), OciPullPolicy::Never)
        .await
        .unwrap();
    assert_eq!(registry.requests.load(Ordering::SeqCst), fetched);
    assert!(
        pull_component(&reference, config, OciPullPolicy::IfNotPresent)
            .await
            .is_err(),
        "a pinned disk hit cannot bypass the current registry check"
    );
    assert!(registry.requests.load(Ordering::SeqCst) > fetched);
}

type StoredArtifact = (wash_runtime::cache::Pulled, [u8; 32]);

/// An external backend needs no filesystem paths or private runtime types.
#[derive(Default)]
struct MemoryArtifacts {
    entries: std::sync::Mutex<std::collections::HashMap<[u8; 32], StoredArtifact>>,
    reads: std::sync::atomic::AtomicUsize,
    writes: std::sync::atomic::AtomicUsize,
    refreshes: std::sync::atomic::AtomicUsize,
    completed: wash_runtime::cache::DefaultOciCache,
}

#[async_trait::async_trait]
impl wash_runtime::cache::OciCache for MemoryArtifacts {
    async fn oci_get_artifact(
        &self,
        key: &wash_runtime::cache::ArtifactKey,
    ) -> anyhow::Result<Option<wash_runtime::cache::ArtifactEntry>> {
        self.reads.fetch_add(1, Ordering::SeqCst);
        // Production backends generate a secret and protect it with the store.
        let storage = key.storage_key(&[42; 32])?;
        let proof = key.credential_proof(&[42; 32])?;
        Ok(self
            .entries
            .lock()
            .unwrap()
            .get(&storage)
            .map(
                |(artifact, credential)| wash_runtime::cache::ArtifactEntry {
                    artifact: artifact.clone(),
                    credentials_match: *credential == proof,
                },
            ))
    }
    async fn oci_put_artifact(
        &self,
        key: &wash_runtime::cache::ArtifactKey,
        value: &wash_runtime::cache::Pulled,
    ) -> anyhow::Result<()> {
        self.entries.lock().unwrap().insert(
            key.storage_key(&[42; 32])?,
            (value.clone(), key.credential_proof(&[42; 32])?),
        );
        self.writes.fetch_add(1, Ordering::SeqCst);
        Ok(())
    }
    async fn oci_record_credentials(
        &self,
        key: &wash_runtime::cache::ArtifactKey,
    ) -> anyhow::Result<()> {
        if let Some((_, proof)) = self
            .entries
            .lock()
            .unwrap()
            .get_mut(&key.storage_key(&[42; 32])?)
        {
            *proof = key.credential_proof(&[42; 32])?;
            self.refreshes.fetch_add(1, Ordering::SeqCst);
        }
        Ok(())
    }
    async fn oci_get_pull(
        &self,
        key: &wash_runtime::cache::PullKey,
    ) -> anyhow::Result<Option<std::sync::Arc<wash_runtime::cache::Pulled>>> {
        self.completed.oci_get_pull(key).await
    }
    async fn oci_put_pull(
        &self,
        key: &wash_runtime::cache::PullKey,
        value: std::sync::Arc<wash_runtime::cache::Pulled>,
    ) -> anyhow::Result<()> {
        self.completed.oci_put_pull(key, value).await
    }
}

#[tokio::test]
async fn custom_artifacts_support_offline_reuse_and_rotation_without_disk() {
    wash_runtime::init_crypto();
    let registry = Registry::start().await;
    let directory = tempfile::tempdir().unwrap();
    let forbidden = directory.path().join("unused");
    let cache = std::sync::Arc::new(MemoryArtifacts::default());
    let config = |password: &str| OciConfig {
        credentials: Some(("user".into(), password.into())),
        insecure: true,
        cache_dir: Some(forbidden.clone()),
        cache: Some(cache.clone()),
        timeout: Some(Duration::from_secs(5)),
    };
    let reference = registry.reference();
    pull_component(&reference, config("secret"), OciPullPolicy::IfNotPresent)
        .await
        .unwrap();
    let before = registry.requests.load(Ordering::SeqCst);
    pull_component(&reference, config("secret"), OciPullPolicy::Never)
        .await
        .unwrap();
    assert_eq!(registry.requests.load(Ordering::SeqCst), before);
    assert!(
        pull_component(&reference, config("wrong"), OciPullPolicy::Never)
            .await
            .is_err()
    );
    assert!(
        pull_component(&reference, config("wrong"), OciPullPolicy::IfNotPresent)
            .await
            .is_err()
    );
    *registry.authorization.write().unwrap() = "Basic dXNlcjpyb3RhdGVk".into();
    pull_component(&reference, config("rotated"), OciPullPolicy::IfNotPresent)
        .await
        .unwrap();
    pull_component(&reference, config("rotated"), OciPullPolicy::Never)
        .await
        .unwrap();
    assert_eq!(cache.writes.load(Ordering::SeqCst), 1);
    assert_eq!(cache.refreshes.load(Ordering::SeqCst), 1);
    assert!(!forbidden.exists());
}

#[tokio::test]
async fn completed_pulls_do_not_bypass_a_different_backend() {
    wash_runtime::init_crypto();
    let registry = Registry::start().await;
    let a = std::sync::Arc::new(MemoryArtifacts::default());
    let b = std::sync::Arc::new(MemoryArtifacts::default());
    let config = |cache: std::sync::Arc<MemoryArtifacts>| OciConfig {
        credentials: Some(("user".into(), "secret".into())),
        insecure: true,
        cache: Some(cache),
        timeout: Some(Duration::from_secs(5)),
        ..Default::default()
    };
    let reference = registry.reference();
    pull_component(&reference, config(a.clone()), OciPullPolicy::IfNotPresent)
        .await
        .unwrap();
    pull_component(&reference, config(b.clone()), OciPullPolicy::IfNotPresent)
        .await
        .unwrap();
    assert_eq!(a.writes.load(Ordering::SeqCst), 1);
    assert_eq!(b.writes.load(Ordering::SeqCst), 1);
    pull_component(&reference, config(a.clone()), OciPullPolicy::IfNotPresent)
        .await
        .unwrap();
    assert_eq!(
        a.reads.load(Ordering::SeqCst),
        1,
        "completed reuse belongs to the selected backend"
    );
}

#[tokio::test]
async fn custom_cache_io_is_inside_the_pull_deadline() {
    struct Pending;
    #[async_trait::async_trait]
    impl wash_runtime::cache::OciCache for Pending {
        async fn oci_get_artifact(
            &self,
            _: &wash_runtime::cache::ArtifactKey,
        ) -> anyhow::Result<Option<wash_runtime::cache::ArtifactEntry>> {
            std::future::pending().await
        }
    }
    let config = OciConfig {
        credentials: Some(("user".into(), "secret".into())),
        cache: Some(std::sync::Arc::new(Pending)),
        timeout: Some(Duration::from_millis(50)),
        ..Default::default()
    };
    let error = pull_component(
        "registry.invalid/ns/component:v1",
        config,
        OciPullPolicy::Never,
    )
    .await
    .unwrap_err();
    assert!(
        format!("{error:#}").contains("timeout pulling component"),
        "{error:#}"
    );
}

#[tokio::test]
async fn credential_cache_hits_are_used_and_errors_do_not_fail_the_pull() {
    struct Credentials {
        fail: bool,
    }
    #[async_trait::async_trait]
    impl wash_runtime::cache::OciCache for Credentials {
        async fn oci_get_credentials(
            &self,
            _: &wash_runtime::cache::CredentialKey,
        ) -> anyhow::Result<Option<wash_runtime::cache::RegistryAuth>> {
            anyhow::ensure!(!self.fail, "credential backend unavailable");
            Ok(Some(wash_runtime::cache::RegistryAuth::Basic(
                "user".into(),
                "secret".into(),
            )))
        }
    }
    wash_runtime::init_crypto();
    let registry = Registry::start().await;
    let config = |fail| OciConfig {
        cache: Some(std::sync::Arc::new(Credentials { fail })),
        insecure: true,
        timeout: Some(Duration::from_secs(5)),
        ..Default::default()
    };
    pull_component(&registry.reference(), config(false), OciPullPolicy::Always)
        .await
        .unwrap();
    // A backend that cannot answer is a miss: the pull falls back to the docker
    // helper and then to anonymous access, and is settled by the registry.
    // This one requires credentials, so it is the registry that refuses.
    let requests = registry.requests.load(Ordering::SeqCst);
    let error = pull_component(&registry.reference(), config(true), OciPullPolicy::Always)
        .await
        .unwrap_err();
    assert!(
        !format!("{error:#}").contains("credential backend unavailable"),
        "{error:#}"
    );
    assert!(registry.requests.load(Ordering::SeqCst) > requests);
}

/// Completed-pull reuse is an optimisation: a backend that fails to serve or
/// keep it costs registry work, never the pull.
#[tokio::test]
async fn completed_pull_cache_errors_do_not_fail_the_pull() {
    struct Failing;
    #[async_trait::async_trait]
    impl wash_runtime::cache::OciCache for Failing {
        async fn oci_get_pull(
            &self,
            _: &wash_runtime::cache::PullKey,
        ) -> anyhow::Result<Option<std::sync::Arc<wash_runtime::cache::Pulled>>> {
            anyhow::bail!("completed pull backend unavailable")
        }
        async fn oci_put_pull(
            &self,
            _: &wash_runtime::cache::PullKey,
            _: std::sync::Arc<wash_runtime::cache::Pulled>,
        ) -> anyhow::Result<()> {
            anyhow::bail!("completed pull backend unavailable")
        }
    }
    wash_runtime::init_crypto();
    let registry = Registry::start().await;
    let config = OciConfig {
        credentials: Some(("user".into(), "secret".into())),
        cache: Some(std::sync::Arc::new(Failing)),
        insecure: true,
        timeout: Some(Duration::from_secs(5)),
        ..Default::default()
    };
    let (bytes, _) = pull_component(&registry.reference(), config, OciPullPolicy::IfNotPresent)
        .await
        .unwrap();
    assert_eq!(bytes, registry.image.read().unwrap().wasm);
}
