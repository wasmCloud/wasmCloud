//! NOTE: to run the tests in this file, you must start a local instance
//! of MinIO (or some other S3-compatible object store equivalent) to use.
//!
//! For example, with docker, you can start MinIO:
//!
//! ```console
//! docker run --rm \
//! -e MINIO_ROOT_USER=minioadmin \
//! -e MINIO_ROOT_PASSWORD=minioadmin \
//! -p 9000:9000 \
//! -p 9001:9001 \
//! bitnami/minio
//! ```
//!
//! When running the tests, you must set the correct ENV variable to influence the
//! creation of the test client (see `test_client()`), for example:
//!
//! ```console
//! export AWS_ENDPOINT=http://localhost:9000
//! export AWS_ACCESS_KEY_ID=minioadmin
//! export AWS_SECRET_ACCESS_KEY=minioadmin
//! cargo test test_create_container -- --nocapture
//! ```
//!
//! To see warnings, make sure add & enable `tracing_subscriber` in the appropriate test(s):
//!
//! ```rust
//! tracing_subscriber::fmt().init()
//! ```

use std::collections::HashMap;
use std::env;

use wasmcloud_provider_blobstore_s3::{StorageClient, StorageConfig};

/// Helper function to create a StorageClient with local testing overrides
async fn test_client() -> StorageClient {
    let conf = StorageConfig {
        endpoint: env::var("AWS_ENDPOINT").ok(),
        access_key_id: env::var("AWS_ACCESS_KEY_ID").ok(),
        secret_access_key: env::var("AWS_SECRET_ACCESS_KEY").ok(),
        aliases: HashMap::new(),
        max_attempts: None,
        region: Some("local".into()),
        session_token: None,
        sts_config: None,
    };

    StorageClient::new(conf, &HashMap::new()).await
}

/// Tests
/// - create_container
/// - remove_container
/// - container_exists
#[tokio::test]
async fn test_create_container() {
    let s3 = test_client().await;

    let num = rand::random::<u64>();
    let bucket = format!("test.bucket.{}", num);

    assert!(
        !s3.container_exists(&bucket).await.unwrap(),
        "Container should not exist"
    );
    s3.create_container(&bucket).await.unwrap();

    assert!(
        s3.container_exists(&bucket).await.unwrap(),
        "Container should exist"
    );
}
