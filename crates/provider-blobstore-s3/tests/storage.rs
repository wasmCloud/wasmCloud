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

use anyhow::{Context as _, Result};
use wasmcloud_provider_blobstore_s3::{StorageClient, StorageConfig};
use wasmcloud_test_util::testcontainers::{AsyncRunner as _, ContainerAsync, ImageExt, LocalStack};

struct TestEnv {
    _container: Option<ContainerAsync<LocalStack>>,
    endpoint: String,
}

impl TestEnv {
    pub async fn new() -> Result<Self> {
        let (endpoint, container) = if let Ok(ep) = env::var("AWS_ENDPOINT") {
            (ep, None)
        } else {
            let node = LocalStack::default()
                .with_env_var("SERVICES", "s3")
                .start()
                .await
                .context("should have started localstack")?;
            let host_ip = node
                .get_host()
                .await
                .context("should have gotten localstack ip")?;
            let host_port = node
                .get_host_port_ipv4(4566)
                .await
                .context("should have gotten localstack port")?;
            (format!("http://{host_ip}:{host_port}"), Some(node))
        };

        Ok(Self {
            endpoint,
            _container: container,
        })
    }

    pub async fn configure_test_client(&self) -> StorageClient {
        let conf = StorageConfig {
            endpoint: Some(self.endpoint.clone()),
            access_key_id: Self::env_var_or_default("AWS_ACCESS_KEY_ID", Some("test".to_string())),
            secret_access_key: Self::env_var_or_default(
                "AWS_SECRET_ACCESS_KEY",
                Some("test".to_string()),
            ),
            aliases: HashMap::new(),
            max_attempts: None,
            region: Self::env_var_or_default("AWS_REGION", Some("us-east-1".to_string())),
            session_token: None,
            sts_config: None,
            bucket_region: Self::env_var_or_default("BUCKET_REGION", None),
        };

        StorageClient::new(conf, &HashMap::new()).await
    }

    fn env_var_or_default(key: &str, default: Option<String>) -> Option<String> {
        std::env::var(key).ok().or(default)
    }
}

/// Tests
/// - create_container
/// - container_exists
#[tokio::test]
async fn test_create_container() {
    let env = TestEnv::new()
        .await
        .expect("should have setup the test environment");

    let s3 = env.configure_test_client().await;

    let num = rand::random::<u64>();
    let bucket = format!("test.bucket.{num}");

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
