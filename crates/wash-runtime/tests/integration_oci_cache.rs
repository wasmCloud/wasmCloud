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
