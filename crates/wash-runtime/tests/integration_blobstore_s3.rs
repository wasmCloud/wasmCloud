#![cfg(all(
    feature = "wasi-blobstore",
    feature = "wasm_component_model_implements"
))]
//! End-to-end test for the multiplexed `wasi:blobstore` S3 backend against a
//! real S3 API (S3Mock), through `MultiplexedAsyncBlobstore`'s
//! provider/registry path.
//!
//! Buckets are pre-created by the container: the backend cannot create or
//! delete them, and the test asserts that too. Beyond the shared
//! `BlobBackend` surface it pins what is S3-specific: pooling by config,
//! `NoSuchContainer` vs `NoSuchObject` from a 404, the clamped range read,
//! cross-bucket copy, bulk delete, and object-name validation.
//!
//! Requires Docker (S3Mock); marked `#[ignore]`, so it runs only under
//! `cargo test -- --ignored` (CI's Linux leg) and not a plain `cargo test`.
#![allow(clippy::unwrap_used, clippy::expect_used)]

use std::collections::HashMap;
use std::sync::Arc;

use anyhow::{Context, Result};
use testcontainers::{
    GenericImage, ImageExt,
    core::{IntoContainerPort, WaitFor},
    runners::AsyncRunner,
};

use wash_runtime::plugin::wasi_blobstore::{
    BlobBackendError, MultiplexedAsyncBlobstore, S3BlobProvider,
};
use wash_runtime::wit::WitInterface;

/// A named `wasmcloud:blobstore` interface routed to the S3 backend.
fn s3_blob_iface(name: &str, endpoint: &str) -> WitInterface {
    WitInterface {
        namespace: "wasmcloud".to_string(),
        package: "blobstore".to_string(),
        interfaces: ["blobstore".to_string(), "container".to_string()]
            .into_iter()
            .collect(),
        version: None,
        config: HashMap::from([
            ("backend".to_string(), "s3".to_string()),
            ("endpoint".to_string(), endpoint.to_string()),
            ("region".to_string(), "us-east-1".to_string()),
            ("allow_http".to_string(), "true".to_string()),
            // S3Mock checks no signature, but without static keys object_store
            // would go looking for instance credentials.
            ("access_key_id".to_string(), "test".to_string()),
            ("secret_access_key".to_string(), "test".to_string()),
        ]),
        name: Some(name.to_string()),
    }
}

/// `BlobBackendError` is not `std::error::Error`; stringify it for `?`/`anyhow`.
fn err(e: impl std::fmt::Debug) -> anyhow::Error {
    anyhow::anyhow!("blobstore backend error: {e:?}")
}

#[tokio::test]
#[ignore = "requires Docker (S3Mock); run with `cargo test -- --ignored`"]
async fn multiplexed_blobstore_routes_to_s3() -> Result<()> {
    // --- S3Mock container, with the buckets the backend cannot create ---
    let s3 = GenericImage::new("adobe/s3mock", "5.2.3")
        .with_exposed_port(9090.tcp())
        .with_wait_for(WaitFor::message_on_stdout("Started S3MockApplication"))
        .with_env_var(
            "COM_ADOBE_TESTING_S3MOCK_STORE_INITIAL_BUCKETS",
            "photos,archive",
        )
        .start()
        .await
        .map_err(|e| anyhow::anyhow!("failed to start s3mock: {e}"))?;
    let endpoint = format!("http://127.0.0.1:{}", s3.get_host_port_ipv4(9090).await?);

    // --- registry: two names, one config, so they must share a backend ---
    let plugin = MultiplexedAsyncBlobstore::new().with_provider(Arc::new(S3BlobProvider));
    let registry = plugin
        .build_registry(&[
            s3_blob_iface("s3-a", &endpoint),
            s3_blob_iface("s3-b", &endpoint),
        ])
        .await
        .context("build registry")?;
    let be = registry.get("s3-a").expect("s3-a routed").clone();
    let other = registry.get("s3-b").expect("s3-b routed");
    assert!(
        Arc::ptr_eq(&be, other),
        "same config must resolve to one pooled backend"
    );

    // --- containers are pre-provisioned; the backend only observes them ---
    assert!(be.container_exists("photos").await.map_err(err)?);
    be.get_container("photos").await.map_err(err)?;
    assert_eq!(
        be.container_info("photos").await.map_err(err)?.name,
        "photos"
    );
    assert!(
        matches!(
            be.create_container("photos").await,
            Err(BlobBackendError::ContainerAlreadyExists(_))
        ),
        "an existing bucket is reported as such, so create-then-get works"
    );
    // Whether the probe says "absent" or fails (object_store#851), the guest
    // must read the guidance first and the probe's own error after it.
    let msg = be
        .create_container("nope")
        .await
        .expect_err("a missing bucket cannot be created here")
        .to_string();
    assert!(
        msg.starts_with("s3 blobstore cannot create buckets; create 'nope' outside wasmCloud"),
        "{msg}"
    );
    assert!(be.delete_container("photos").await.is_err());
    // A missing bucket is never reported as present. Whether it is `false`
    // or an error depends on object_store (apache/arrow-rs-object-store#851).
    assert!(!matches!(be.container_exists("nope").await, Ok(true)));
    assert!(matches!(
        be.get_container("nope").await,
        Err(BlobBackendError::NoSuchContainer(_) | BlobBackendError::Other(_))
    ));

    // --- object write / read round-trip, with the clamped inclusive range ---
    be.write_data("photos", "cat.png", b"meow".to_vec())
        .await
        .map_err(err)?;
    assert!(be.has_object("photos", "cat.png").await.map_err(err)?);
    let read = |start, end| be.get_data("photos", "cat.png", start, end);
    assert_eq!(read(0, u64::MAX).await.map_err(err)?, b"meow");
    assert_eq!(read(0, 1).await.map_err(err)?, b"me");
    assert_eq!(read(2, u64::MAX).await.map_err(err)?, b"ow");
    assert_eq!(read(2, 2).await.map_err(err)?, b"o");
    assert!(
        read(4, u64::MAX).await.map_err(err)?.is_empty(),
        "a range starting past the end reads nothing, not an error"
    );
    let info = be.object_info("photos", "cat.png").await.map_err(err)?;
    assert_eq!(info.size, 4);
    assert_eq!(info.container, "photos");
    assert!(
        info.created_at > 0,
        "last-modified stands in for created_at"
    );

    be.write_data("photos", "empty", Vec::new())
        .await
        .map_err(err)?;
    assert!(
        be.get_data("photos", "empty", 0, u64::MAX)
            .await
            .map_err(err)?
            .is_empty()
    );
    assert_eq!(
        be.object_info("photos", "empty").await.map_err(err)?.size,
        0
    );

    // --- a missing object is NoSuchObject, not a generic failure ---
    assert!(!be.has_object("photos", "missing").await.map_err(err)?);
    assert!(matches!(
        be.get_data("photos", "missing", 0, u64::MAX).await,
        Err(BlobBackendError::NoSuchObject(_))
    ));
    assert!(matches!(
        be.object_info("photos", "missing").await,
        Err(BlobBackendError::NoSuchObject(_))
    ));

    // --- listing, including a nested key ---
    be.write_data("photos", "a/b.txt", b"nested".to_vec())
        .await
        .map_err(err)?;
    let mut names = be.list_objects("photos").await.map_err(err)?;
    names.sort();
    assert_eq!(names, ["a/b.txt", "cat.png", "empty"]);

    // --- copy: server-side within a bucket, get+put across buckets ---
    be.copy_object("photos", "cat.png", "photos", "cat-copy.png")
        .await
        .map_err(err)?;
    assert_eq!(
        be.get_data("photos", "cat-copy.png", 0, u64::MAX)
            .await
            .map_err(err)?,
        b"meow"
    );
    be.copy_object("photos", "cat.png", "archive", "cat.png")
        .await
        .map_err(err)?;
    assert_eq!(
        be.get_data("archive", "cat.png", 0, u64::MAX)
            .await
            .map_err(err)?,
        b"meow"
    );
    assert!(matches!(
        be.copy_object("photos", "missing", "photos", "x").await,
        Err(BlobBackendError::NoSuchObject(_))
    ));
    be.move_object("photos", "cat-copy.png", "archive", "moved.png")
        .await
        .map_err(err)?;
    assert!(!be.has_object("photos", "cat-copy.png").await.map_err(err)?);
    assert!(be.has_object("archive", "moved.png").await.map_err(err)?);

    // --- delete: single (idempotent), bulk (tolerates missing), empty ---
    be.delete_object("photos", "a/b.txt").await.map_err(err)?;
    assert!(!be.has_object("photos", "a/b.txt").await.map_err(err)?);
    be.delete_object("photos", "a/b.txt").await.map_err(err)?;
    be.delete_objects("photos", &["empty".to_string(), "missing".to_string()])
        .await
        .map_err(err)?;
    assert!(!be.has_object("photos", "empty").await.map_err(err)?);
    be.delete_objects("photos", &[]).await.map_err(err)?;
    assert_eq!(be.list_objects("photos").await.map_err(err)?, ["cat.png"]);

    // --- clear ---
    be.clear_container("archive").await.map_err(err)?;
    assert!(be.list_objects("archive").await.map_err(err)?.is_empty());
    be.clear_container("archive").await.map_err(err)?;

    // --- a 404 on a write is the bucket, not the object ---
    assert!(matches!(
        be.write_data("nope", "x", b"x".to_vec()).await,
        Err(BlobBackendError::NoSuchContainer(_))
    ));
    assert!(matches!(
        be.delete_object("nope", "x").await,
        Err(BlobBackendError::NoSuchContainer(_))
    ));

    // --- object names are validated, not rewritten ---
    for bad in ["a//b", "../x", "a/./b"] {
        assert!(
            be.write_data("photos", bad, b"x".to_vec()).await.is_err(),
            "'{bad}' must be refused"
        );
    }

    Ok(())
}

/// Config for a real bucket, with one of the two credential sources.
fn real_s3_iface(name: &str, region: &str, credentials: &[(&str, &str)]) -> WitInterface {
    let mut config = HashMap::from([
        ("backend".to_string(), "s3".to_string()),
        ("region".to_string(), region.to_string()),
    ]);
    config.extend(
        credentials
            .iter()
            .map(|(k, v)| (k.to_string(), v.to_string())),
    );
    WitInterface {
        namespace: "wasmcloud".to_string(),
        package: "blobstore".to_string(),
        interfaces: ["blobstore".to_string(), "container".to_string()]
            .into_iter()
            .collect(),
        version: None,
        config,
        name: Some(name.to_string()),
    }
}

/// What S3Mock cannot check: that requests this backend builds are signed in
/// a way real AWS accepts, for both credential sources. Needs a dedicated
/// bucket in `WASH_TEST_S3_BUCKET`, `AWS_REGION`, and `AWS_ACCESS_KEY_ID` /
/// `AWS_SECRET_ACCESS_KEY` (plus `AWS_SESSION_TOKEN` if any) in the
/// environment, e.g. from `aws configure export-credentials --format env`.
/// Writes a handful of objects under a unique prefix and removes them.
///
/// Skips, rather than fails, when no bucket is named: CI runs every ignored
/// test and has no AWS account.
#[tokio::test]
#[ignore = "requires AWS credentials and WASH_TEST_S3_BUCKET; run with `cargo test -- --ignored`"]
async fn multiplexed_blobstore_signs_against_real_s3() -> Result<()> {
    let Ok(bucket) = std::env::var("WASH_TEST_S3_BUCKET") else {
        eprintln!(
            "Skipping real S3 test (set WASH_TEST_S3_BUCKET to a dedicated bucket to enable)"
        );
        return Ok(());
    };
    let region = std::env::var("AWS_REGION")
        .or_else(|_| std::env::var("AWS_DEFAULT_REGION"))
        .context("set AWS_REGION")?;
    let key_id = std::env::var("AWS_ACCESS_KEY_ID").context("set AWS_ACCESS_KEY_ID")?;
    let secret = std::env::var("AWS_SECRET_ACCESS_KEY").context("set AWS_SECRET_ACCESS_KEY")?;
    let token = std::env::var("AWS_SESSION_TOKEN").ok();

    // Static keys through the manifest table (token included when present),
    // and the host identity through `from_env`.
    let mut static_keys = vec![
        ("access_key_id", key_id.as_str()),
        ("secret_access_key", secret.as_str()),
    ];
    if let Some(token) = token.as_deref() {
        static_keys.push(("session_token", token));
    }
    let plugin = MultiplexedAsyncBlobstore::new().with_provider(Arc::new(S3BlobProvider));
    let registry = plugin
        .build_registry(&[
            real_s3_iface("static", &region, &static_keys),
            real_s3_iface("host", &region, &[("use_host_identity", "true")]),
        ])
        .await
        .context("build registry")?;

    let run = uuid::Uuid::new_v4().simple().to_string();
    for (label, be) in [("static", &registry["static"]), ("host", &registry["host"])] {
        let prefix = format!("wash-s3-test/{run}/{label}");
        let object = format!("{prefix}/cat.png");
        let copy = format!("{prefix}/cat-copy.png");
        let ctx = |what: &str| format!("[{label}] {what}");

        anyhow::ensure!(
            be.container_exists(&bucket).await.map_err(err)?,
            "{}",
            ctx("bucket must exist")
        );
        // Everything that writes runs in one block with `ensure!` rather than
        // `assert!`, so a failure part-way still reaches the cleanup below and
        // a failed run leaves nothing in the bucket.
        let written = async {
            be.write_data(&bucket, &object, b"meow".to_vec())
                .await
                .map_err(err)
                .context(ctx("write"))?;
            let read = be
                .get_data(&bucket, &object, 0, u64::MAX)
                .await
                .map_err(err)
                .context(ctx("read"))?;
            anyhow::ensure!(read == b"meow", "{}: {read:?}", ctx("read"));
            let range = be
                .get_data(&bucket, &object, 1, 2)
                .await
                .map_err(err)
                .context(ctx("range read"))?;
            anyhow::ensure!(range == b"eo", "{}: {range:?}", ctx("range read"));
            let size = be
                .object_info(&bucket, &object)
                .await
                .map_err(err)
                .context(ctx("head"))?
                .size;
            anyhow::ensure!(size == 4, "{}: size {size}", ctx("head"));
            be.copy_object(&bucket, &object, &bucket, &copy)
                .await
                .map_err(err)
                .context(ctx("copy"))?;
            let listed = be
                .list_objects(&bucket)
                .await
                .map_err(err)
                .context(ctx("list"))?;
            anyhow::ensure!(
                listed.contains(&object) && listed.contains(&copy),
                "{}: {listed:?}",
                ctx("list")
            );
            be.delete_objects(&bucket, &[object.clone(), copy.clone()])
                .await
                .map_err(err)
                .context(ctx("bulk delete"))?;
            anyhow::ensure!(
                !be.has_object(&bucket, &object).await.map_err(err)?,
                "{}",
                ctx("deleted object must be gone")
            );
            anyhow::Ok(())
        }
        .await;
        // Best effort: on success this deletes nothing, on failure it removes
        // whatever the block managed to write before propagating the error.
        let _ = be
            .delete_objects(&bucket, &[object.clone(), copy.clone()])
            .await;
        written?;

        // Real S3 answers a PUT to a missing bucket with 404 NoSuchBucket.
        let missing = format!("wash-s3-test-missing-{run}");
        assert!(
            matches!(
                be.write_data(&missing, "x", b"x".to_vec()).await,
                Err(BlobBackendError::NoSuchContainer(_))
            ),
            "{}",
            ctx("missing bucket must be NoSuchContainer")
        );
        assert!(
            !matches!(be.container_exists(&missing).await, Ok(true)),
            "{}",
            ctx("missing bucket must never be reported present")
        );
        // A bucket that exists but belongs to someone else. S3 answers 403 from
        // its own region and 301 from any other (object_store does not follow
        // the redirect), so the exact error depends on `AWS_REGION`; what is
        // pinned is that it is an error, never "absent", which is why
        // `container_exists` does not round errors to `false` (object_store#851).
        assert!(
            be.container_exists("aws").await.is_err(),
            "{}",
            ctx("someone else's bucket must be an error, not false")
        );
        assert!(
            matches!(
                be.create_container(&bucket).await,
                Err(BlobBackendError::ContainerAlreadyExists(_))
            ),
            "{}",
            ctx("our own bucket must be ContainerAlreadyExists")
        );
    }

    Ok(())
}
