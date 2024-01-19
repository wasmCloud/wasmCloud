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

use wasmcloud_provider_blobstore_s3::{
    Chunk, ContainerObjectSelector, GetObjectRequest, ListObjectsRequest, PutObjectRequest,
    RemoveObjectsRequest, StorageClient, StorageConfig,
};
use wasmcloud_provider_wit_bindgen::deps::wasmcloud_provider_sdk::Context;

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
        max_chunk_size_bytes: None,
        tls_use_webpki_roots: None,
    };

    StorageClient::new(conf, Default::default()).await
}

/// Tests
/// - create_container
/// - remove_container
/// - container_exists
#[tokio::test]
async fn test_create_container() {
    let s3 = test_client().await;
    let ctx = Context::default();

    let num = rand::random::<u64>();
    let bucket = format!("test.bucket.{}", num);

    assert!(
        !s3.container_exists(&ctx, &bucket).await,
        "Container should not exist"
    );
    s3.create_container(&ctx, &bucket).await;

    assert!(
        s3.container_exists(&ctx, &bucket).await,
        "Container should exist"
    );

    assert!(
        s3.remove_containers(&ctx, &vec![bucket])
            .await
            .iter()
            .all(|result| result.success),
        "no errors during removal",
    );
}

/// Tests
/// - put_object
/// - remove_object
/// - object_exists
#[tokio::test]
async fn test_create_object() {
    let s3 = test_client().await;
    let ctx = Context::default();

    let num = rand::random::<u64>();
    let bucket = format!("test.object.{}", num);

    s3.create_container(&ctx, &bucket).await;

    let object_bytes = b"hello-world!".to_vec();
    let resp = s3
        .put_object(
            &ctx,
            &PutObjectRequest {
                chunk: Chunk {
                    bytes: object_bytes.clone(),
                    container_id: bucket.clone(),
                    is_last: true,
                    object_id: "object.1".to_string(),
                    offset: 0,
                },
                content_encoding: None,
                content_type: None,
            },
        )
        .await;
    assert_eq!(resp.stream_id, None);

    assert!(
        s3.object_exists(
            &ctx,
            &ContainerObjectSelector {
                container_id: bucket.clone(),
                object_id: "object.1".to_string(),
            },
        )
        .await,
        "Object should not exist"
    );

    assert!(
        s3.remove_objects(
            &ctx,
            &RemoveObjectsRequest {
                container_id: bucket.clone(),
                objects: vec!["object.1".to_string()],
            },
        )
        .await
        .is_empty(),
        "no errors during object removal",
    );

    assert!(
        !s3.object_exists(
            &ctx,
            &ContainerObjectSelector {
                container_id: bucket.clone(),
                object_id: "object.1".to_string(),
            },
        )
        .await,
        "Object should exist"
    );

    assert!(
        s3.remove_containers(&ctx, &vec![bucket])
            .await
            .iter()
            .all(|result| result.success),
        "no errors while removing containers",
    )
}

/// Tests:
/// - list_objects
#[tokio::test]
async fn test_list_objects() {
    let s3 = test_client().await;
    let ctx = Context::default();

    let num = rand::random::<u64>();
    let bucket = format!("test.list.{}", num);

    s3.create_container(&ctx, &bucket).await;

    let req = ListObjectsRequest {
        container_id: bucket.clone(),
        continuation: None,
        end_before: None,
        end_with: None,
        max_items: None,
        start_with: None,
    };
    let objs = s3.list_objects(&ctx, &req).await;
    assert_eq!(objs.continuation, None);
    assert!(objs.is_last);
    assert_eq!(objs.objects.len(), 0);

    let object_bytes = b"hello-world!".to_vec();
    let resp = s3
        .put_object(
            &ctx,
            &PutObjectRequest {
                chunk: Chunk {
                    bytes: object_bytes.clone(),
                    container_id: bucket.clone(),
                    is_last: true,
                    object_id: "object.1".to_string(),
                    offset: 0,
                },
                content_encoding: None,
                content_type: None,
            },
        )
        .await;
    assert_eq!(resp.stream_id, None);

    let req = ListObjectsRequest {
        container_id: bucket.clone(),
        continuation: None,
        end_before: None,
        end_with: None,
        max_items: None,
        start_with: None,
    };
    let objs = s3.list_objects(&ctx, &req).await;
    let meta = objs.objects.first().unwrap();
    assert_eq!(&meta.container_id, &bucket);
    assert_eq!(meta.content_length as usize, object_bytes.len());
    assert_eq!(&meta.object_id, "object.1");

    // Empty the bucket before attempting to delete it (minio requires this)
    s3.remove_objects(
        &ctx,
        &RemoveObjectsRequest {
            container_id: bucket.clone(),
            objects: vec!["object.1".to_string()],
        },
    )
    .await;

    assert!(
        s3.remove_containers(&ctx, &vec![bucket])
            .await
            .iter()
            .all(|result| result.success),
        "no errors while removing containers"
    );
}

/// Tests
/// - get_object_range
#[tokio::test]
async fn test_get_object_range() {
    let s3 = test_client().await;
    let ctx = Context::default();
    let num = rand::random::<u64>();
    let bucket = format!("test.range.{}", num);

    s3.create_container(&ctx, &bucket).await;
    let object_bytes = b"abcdefghijklmnopqrstuvwxyz".to_vec();
    assert!(
        s3.put_object(
            &ctx,
            &PutObjectRequest {
                chunk: Chunk {
                    bytes: object_bytes.clone(),
                    container_id: bucket.clone(),
                    is_last: true,
                    object_id: "object.1".to_string(),
                    offset: 0,
                },
                content_encoding: None,
                content_type: None,
            },
        )
        .await
        .stream_id
        .is_none(),
        "put object succeeded (stream_id is always missing on success)",
    );

    let range_mid = s3
        .get_object(
            &ctx,
            &GetObjectRequest {
                container_id: bucket.clone(),
                object_id: "object.1".to_string(),
                range_start: Some(6),
                range_end: Some(12),
            },
        )
        .await;
    assert_eq!(range_mid.content_length, 7);
    assert_eq!(
        range_mid.initial_chunk.as_ref().unwrap().bytes,
        b"ghijklm".to_vec()
    );

    // range with omitted end
    let range_to_end = s3
        .get_object(
            &ctx,
            &GetObjectRequest {
                container_id: bucket.clone(),
                object_id: "object.1".to_string(),
                range_start: Some(22),
                range_end: None,
            },
        )
        .await;
    assert_eq!(range_to_end.content_length, 4);
    assert_eq!(
        range_to_end.initial_chunk.as_ref().unwrap().bytes,
        b"wxyz".to_vec()
    );

    // range with omitted begin
    let range_from_start = s3
        .get_object(
            &ctx,
            &GetObjectRequest {
                container_id: bucket.clone(),
                object_id: "object.1".to_string(),
                range_start: None,
                range_end: Some(3),
            },
        )
        .await;
    assert_eq!(
        range_from_start.initial_chunk.as_ref().unwrap().bytes,
        b"abcd".to_vec()
    );
    //assert_eq!(range_from_start.content_length, 4);

    // Empty the bucket before attempting to delete it (minio requires this)
    s3.remove_objects(
        &ctx,
        &RemoveObjectsRequest {
            container_id: bucket.clone(),
            objects: vec!["object.1".to_string()],
        },
    )
    .await;

    assert!(
        s3.remove_containers(&ctx, &vec![bucket])
            .await
            .iter()
            .all(|result| result.success),
        "no errors when removing containers"
    );
}

/// Tests
/// - get_object with chunked response
#[tokio::test]
async fn test_get_object_chunks() {
    let s3 = test_client().await;
    let ctx = Context::default();
    let num = rand::random::<u64>();
    let bucket = format!("test.chunk.{}", num);

    s3.create_container(&ctx, &bucket).await;

    for count in [4, 40, 400, 4000].iter() {
        let fname = format!("file_{}", (count * 25));
        let object_bytes = b"abcdefghijklmnopqrstuvwxy".repeat(*count);
        assert!(
            s3.put_object(
                &ctx,
                &PutObjectRequest {
                    chunk: Chunk {
                        bytes: object_bytes,
                        container_id: bucket.clone(),
                        is_last: true,
                        object_id: fname,
                        offset: 0,
                    },
                    content_encoding: None,
                    content_type: None,
                },
            )
            .await
            .stream_id
            .is_none(),
            "put object succeeded (stream_id is always missing on success)"
        )
    }

    let obj = s3
        .get_object(
            &ctx,
            &GetObjectRequest {
                container_id: bucket.clone(),
                object_id: "file_1000".to_string(),
                range_end: None,
                range_start: None,
            },
        )
        .await;
    assert!(obj.initial_chunk.unwrap().bytes.len() >= 1000);

    env::set_var("MAX_CHUNK_SIZE_BYTES", "300");
    let obj = s3
        .get_object(
            &ctx,
            &GetObjectRequest {
                container_id: bucket.clone(),
                object_id: "file_100000".to_string(),
                range_end: None,
                range_start: None,
            },
        )
        .await;
    assert_eq!(obj.initial_chunk.unwrap().bytes.len(), 300);
}
