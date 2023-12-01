use std::collections::HashMap;
use std::net::Ipv6Addr;
use std::sync::Arc;

use anyhow::{anyhow, Context, Result};
use nkeys::KeyPair;
use serde::Deserialize;
use serde_json::json;
use tokio::fs;
use tokio::time::Duration;
use tokio::try_join;
use url::Url;
use wascap::jwt;

use wascap::wasm::extract_claims;
use wasmcloud_control_interface::ClientBuilder;
use wasmcloud_host::wasmbus::{Host, HostConfig};

pub mod common;
use common::free_port;

use crate::common::minio::start_minio;
use crate::common::nats::start_nats;
use crate::common::{
    assert_advertise_link, assert_start_actor, assert_start_provider, stop_server,
};

const LATTICE_PREFIX: &str = "test-blobstorage-s3";

/// Test all functionality for the blobstore-s3 provider
///
/// - create_container
/// - remove_container
/// - container_exists
#[tokio::test(flavor = "multi_thread")]
async fn blobstore_s3_suite() -> Result<()> {
    // Start a NATS & minio
    let (
        (nats_server, stop_nats_tx, nats_url, nats_client),
        (minio_server, stop_minio_tx, _minio_data_dir, minio_url),
    ) = try_join!(start_nats(), start_minio()).context("failed to start backing services")?;

    let httpserver_port = free_port().await?;
    let httpserver_base_url = format!("http://[{}]:{httpserver_port}", Ipv6Addr::LOCALHOST);

    // Get provider key/url for pre-built httpserver provider
    let httpserver_provider_key = KeyPair::from_seed(test_providers::RUST_HTTPSERVER_SUBJECT)
        .context("failed to parse `rust-httpserver` provider key")?;
    let httpserver_provider_url = Url::from_file_path(test_providers::RUST_HTTPSERVER)
        .expect("failed to construct provider ref");

    // Get provider key/url for pre-built blobstore-s3 provider (subject of this test)
    let blobstore_s3_provider_key = KeyPair::from_seed(test_providers::RUST_BLOBSTORE_S3_SUBJECT)
        .context("failed to parse `rust-blobstore-s3` provider key")?;
    let blobstore_s3_provider_url = Url::from_file_path(test_providers::RUST_BLOBSTORE_S3)
        .map_err(|()| anyhow!("failed to construct provider ref"))?;

    // Get actor key/url for pre-built blobstore-http-smithy actor
    let blobstore_http_smithy_actor_url =
        Url::from_file_path(test_actors::RUST_BLOBSTORE_HTTP_SMITHY_SIGNED)
            .map_err(|()| anyhow!("failed to construct actor ref"))?;

    // Build client for interacting with the lattice
    let ctl_client = ClientBuilder::new(nats_client.clone())
        .lattice_prefix(LATTICE_PREFIX.to_string())
        .build();

    // Start a wasmcloud host
    let cluster_key = Arc::new(KeyPair::new_cluster());
    let host_key = Arc::new(KeyPair::new_server());
    let (_host, shutdown_host) = Host::new(HostConfig {
        ctl_nats_url: nats_url.clone(),
        rpc_nats_url: nats_url.clone(),
        lattice_prefix: LATTICE_PREFIX.into(),
        cluster_key: Some(Arc::clone(&cluster_key)),
        cluster_issuers: Some(vec![cluster_key.public_key(), cluster_key.public_key()]),
        host_key: Some(Arc::clone(&host_key)),
        provider_shutdown_delay: Some(Duration::from_millis(300)),
        allow_file_load: true,
        ..Default::default()
    })
    .await
    .context("failed to initialize host")?;

    // Retrieve claims from actor
    let jwt::Token {
        claims: blobstore_http_smithy_claims,
        ..
    } = extract_claims(fs::read(test_actors::RUST_BLOBSTORE_HTTP_SMITHY_SIGNED).await?)
        .context("failed to extract blobstore s3 http smithy actor claims")?
        .context("component actor claims missing")?;

    // Link the actor to both providers
    //
    // this must be done *before* the provider is started to avoid a race condition
    // to ensure the link is advertised before the actor would normally subscribe
    assert_advertise_link(
        &ctl_client,
        &blobstore_http_smithy_claims,
        &httpserver_provider_key,
        "wasmcloud:httpserver",
        "default",
        HashMap::from([(
            "config_json".into(),
            format!(
                r#"{{"address":"[{}]:{httpserver_port}"}}"#,
                Ipv6Addr::LOCALHOST,
            ),
        )]),
    )
    .await?;
    assert_advertise_link(
        &ctl_client,
        &blobstore_http_smithy_claims,
        &blobstore_s3_provider_key,
        "wasmcloud:blobstore",
        "default",
        HashMap::from([(
            "config_json".into(),
            serde_json::to_string(&json!({
                "access_key_id": "minioadmin",
                "secret_access_key": "minioadmin",
                "region": "minio",
                "endpoint": minio_url,
                "max_chunk_size_bytes": 300,
                "tls_use_webpki_roots": true,
            }))?,
        )]),
    )
    .await?;

    // Start the blobstore-http-smithy actor
    assert_start_actor(
        &ctl_client,
        &nats_client,
        LATTICE_PREFIX,
        &host_key,
        blobstore_http_smithy_actor_url,
        1,
    )
    .await?;

    // Start the HTTP provider
    assert_start_provider(
        &ctl_client,
        &nats_client,
        LATTICE_PREFIX,
        &host_key,
        &httpserver_provider_key,
        "default",
        httpserver_provider_url,
        None,
    )
    .await?;

    // Start the blobstore-s3 provider
    assert_start_provider(
        &ctl_client,
        &nats_client,
        LATTICE_PREFIX,
        &host_key,
        &blobstore_s3_provider_key,
        "default",
        blobstore_s3_provider_url,
        None,
    )
    .await?;

    let http_client = reqwest::Client::default();

    // Test operations on containers
    test_ops_containers(&httpserver_base_url, &http_client).await?;

    // Test operations on objects
    test_ops_objects(&httpserver_base_url, &http_client).await?;

    // Test operations on objects
    test_ops_object_range(&httpserver_base_url, &http_client).await?;

    // Test operations on objects
    test_ops_object_chunking(&httpserver_base_url, &http_client).await?;

    // Shutdown the host and backing services
    shutdown_host.await?;
    try_join!(
        stop_server(minio_server, stop_minio_tx),
        stop_server(nats_server, stop_nats_tx),
    )
    .context("failed to stop servers")?;

    Ok(())
}

/// Helper function that tests operations on objects:
///
/// - put_object
/// - object_exists
/// - list_objects
/// - remove_objects
/// - remove_objects
///
/// In order to perform these tests, this function assumes you have a:
///
/// - running httpserver provider, accessible at `base_url`
/// - running blobstore-http-smithy actor, connected to that httpserver provider
async fn test_ops_containers(
    base_url: impl AsRef<str>,
    http_client: &reqwest::Client,
) -> Result<()> {
    let base_url = base_url.as_ref();
    let container = "container";

    // Perform POST request to trigger a blobstore create-container
    let resp_json: ResponseEnvelope<Option<()>> = http_client
        .post(format!("{base_url}/create-container"))
        .body(serde_json::to_string(&json!({"name": container }))?)
        .send()
        .await
        .context("failed to perform POST /create-container")?
        .json()
        .await
        .context("failed to read /create-container response body as json")?;
    assert_eq!(resp_json.status, "success", "create-container succeeded");

    // Perform POST request to trigger a blobstore container-exists
    let resp_json: ResponseEnvelope<bool> = http_client
        // let resp_json: String = http_client
        .post(format!("{base_url}/container-exists"))
        .body(serde_json::to_string(&json!({"name": container }))?)
        .send()
        .await
        .context("failed to perform POST /container-exists")?
        .json()
        .await
        .context("failed to read /container-exists response body as json")?;
    assert_eq!(resp_json.status, "success", "container-exists succeeded");
    assert!(resp_json.data, "container does exist");

    // Perform POST request to trigger a blobstore remove-container
    let resp_json: ResponseEnvelope<Vec<OperationResult>> = http_client
        .post(format!("{base_url}/remove-containers"))
        .body(serde_json::to_string(&json!({"names": vec![container] }))?)
        .send()
        .await
        .context("failed to perform POST /remove-containers")?
        .json()
        .await
        .context("failed to read /remove-containers response body as json")?;
    assert_eq!(resp_json.status, "success", "initial get succeeded");
    assert_eq!(
        resp_json.data[..],
        [OperationResult {
            key: container.into(),
            error: None,
            success: true,
        }],
        "operation results match expectations"
    );

    // Perform POST request to trigger a blobstore container-exists (post-delete)
    let resp_json: ResponseEnvelope<bool> = http_client
        .post(format!("{base_url}/container-exists"))
        .body(serde_json::to_string(&json!({"name": container }))?)
        .send()
        .await
        .context("failed to perform POST /container-exists (post-delete)")?
        .json()
        .await
        .context("failed to read /container-exists response body as json")?;
    assert_eq!(resp_json.status, "success", "container-exists succeeded");
    assert!(!resp_json.data, "container no longer exists");
    Ok(())
}

/// Helper function that tests operations on objects:
///
/// - put_object
/// - object_exists
/// - list_objects
/// - remove_objects
/// - remove_objects
///
/// In order to perform these tests, this function assumes you have a:
///
/// - running httpserver provider, accessible at `base_url`
/// - running blobstore-http-smithy actor, connected to that httpserver provider
async fn test_ops_objects(base_url: impl AsRef<str>, http_client: &reqwest::Client) -> Result<()> {
    let base_url = base_url.as_ref();
    let num = rand::random::<u64>();
    let container = format!("test.object.{}", num);
    let object = format!("object.{}", num);
    let object_bytes = b"hello-world!".to_vec();
    let object_bytes_len = object_bytes.len();

    // Create a container to house the test object(s)
    let resp_json: ResponseEnvelope<Option<()>> = http_client
        .post(format!("{base_url}/create-container"))
        .body(serde_json::to_string(&json!({"name": container }))?)
        .send()
        .await
        .context("failed to perform POST /create-container")?
        .json()
        .await
        .context("failed to read /create-container response body as json")?;
    assert_eq!(resp_json.status, "success", "create-container succeeded");

    // Put the object bytes
    let resp_json: ResponseEnvelope<PutObjectResponse> = http_client
        .post(format!("{base_url}/put-object/{container}/{object}"))
        .body(object_bytes)
        .send()
        .await
        .context("failed to perform POST /put-object")?
        .json()
        .await
        .context("failed to read /put-object response body as json")?;
    assert_eq!(resp_json.status, "success", "put-object succeeded");

    // Check that object exists
    let resp_json: ResponseEnvelope<bool> = http_client
        .post(format!("{base_url}/object-exists"))
        .body(serde_json::to_string(
            &json!({ "containerId": container, "objectId": object }),
        )?)
        .send()
        .await
        .context("failed to perform POST /object-exists (post-put)")?
        .json()
        .await
        .context("failed to read /object-exists response body as json")?;
    assert_eq!(resp_json.status, "success", "object-exists succeeded");
    assert!(resp_json.data, "object exists");

    // Check that object can be listed
    let resp_json: ResponseEnvelope<ListObjectsResponse> = http_client
        .post(format!("{base_url}/list-objects"))
        .body(serde_json::to_string(&json!({ "containerId": container }))?)
        .send()
        .await
        .context("failed to perform POST /list-objects (post-put)")?
        .json()
        .await
        .context("failed to read /list-objects response body as json")?;
    assert_eq!(resp_json.status, "success", "list-objects succeeded");
    let meta = resp_json
        .data
        .objects
        .get(0)
        .context("failed to get first element listed objects")?;
    assert_eq!(&meta.container_id, &container);
    assert_eq!(meta.content_length as usize, object_bytes_len);
    assert_eq!(&meta.object_id, &object);

    // Remove the object
    let resp_json: ResponseEnvelope<Vec<OperationResult>> = http_client
        .post(format!("{base_url}/remove-objects"))
        .body(serde_json::to_string(
            &json!({ "containerId": container, "objects": [object] }),
        )?)
        .send()
        .await
        .context("failed to perform POST /remove-objects")?
        .json()
        .await
        .context("failed to read /remove-objects response body as json")?;
    assert_eq!(resp_json.status, "success", "remove-objects succeeded");

    // Check that object no longer exists
    let resp_json: ResponseEnvelope<bool> = http_client
        .post(format!("{base_url}/object-exists"))
        .body(serde_json::to_string(
            &json!({ "containerId": container, "objectId": object }),
        )?)
        .send()
        .await
        .context("failed to perform POST /object-exists (post-remove-objects)")?
        .json()
        .await
        .context("failed to read /object-exists response body as json")?;
    assert_eq!(resp_json.status, "success", "object-exists succeeded");
    assert!(!resp_json.data, "object no longer exists");

    Ok(())
}

/// Helper function that tests operations on object ranges:
///
/// - put_object
/// - get_object (w/ range specified)
///
/// In order to perform these tests, this function assumes you have a:
///
/// - running httpserver provider, accessible at `base_url`
/// - running blobstore-http-smithy actor, connected to that httpserver provider
async fn test_ops_object_range(
    base_url: impl AsRef<str>,
    http_client: &reqwest::Client,
) -> Result<()> {
    let base_url = base_url.as_ref();
    let num = rand::random::<u64>();
    let container = format!("test.object-range.{}", num);
    let object = format!("object.{}", num);
    let object_bytes = b"abcdefghijklmnopqrstuvwxyz".to_vec();

    // Create a container to house the test object(s)
    let resp_json: ResponseEnvelope<Option<()>> = http_client
        .post(format!("{base_url}/create-container"))
        .body(serde_json::to_string(&json!({"name": container }))?)
        .send()
        .await
        .context("failed to perform POST /create-container")?
        .json()
        .await
        .context("failed to read /create-container response body as json")?;
    assert_eq!(resp_json.status, "success", "create-container succeeded");

    // Put the object bytes
    let resp_json: ResponseEnvelope<PutObjectResponse> = http_client
        .post(format!("{base_url}/put-object/{container}/{object}"))
        .body(object_bytes)
        .send()
        .await
        .context("failed to perform POST /put-object")?
        .json()
        .await
        .context("failed to read /put-object response body as json")?;
    assert_eq!(resp_json.status, "success", "put-object succeeded");

    // Test the middle of the range
    let resp_json: ResponseEnvelope<GetObjectResponse> = http_client
        .post(format!("{base_url}/get-object"))
        .body(serde_json::to_string(&json!({
            "containerId": container,
            "objectId": object,
            "rangeStart": 6,
            "rangeEnd": 12,
        }))?)
        .send()
        .await
        .context("failed to perform POST /get-object")?
        .json()
        .await
        .context("failed to read /get-object response body as json")?;
    assert_eq!(
        resp_json.status, "success",
        "get-object (range mid) succeeded"
    );
    assert_eq!(resp_json.data.content_length, 7);
    assert_eq!(
        resp_json.data.initial_chunk.as_ref().unwrap().bytes,
        b"ghijklm".to_vec()
    );

    // Test the end of the range w/ explicit end omitted
    let resp_json: ResponseEnvelope<GetObjectResponse> = http_client
        .post(format!("{base_url}/get-object"))
        .body(serde_json::to_string(&json!({
            "containerId": container,
            "objectId": object,
            "rangeStart": 22,
            "rangeEnd": null,
        }))?)
        .send()
        .await
        .context("failed to perform POST /get-object")?
        .json()
        .await
        .context("failed to read /get-object response body as json")?;
    assert_eq!(
        resp_json.status, "success",
        "get-object (range end) succeeded"
    );
    assert_eq!(resp_json.data.content_length, 4);
    assert_eq!(
        resp_json.data.initial_chunk.as_ref().unwrap().bytes,
        b"wxyz".to_vec()
    );

    // Test the beginning of the range w/ explicit begin omitted
    let resp_json: ResponseEnvelope<GetObjectResponse> = http_client
        .post(format!("{base_url}/get-object"))
        .body(serde_json::to_string(&json!({
            "containerId": container,
            "objectId": object,
            "rangeStart": null,
            "rangeEnd": 3,
        }))?)
        .send()
        .await
        .context("failed to perform POST /get-object")?
        .json()
        .await
        .context("failed to read /get-object response body as json")?;
    assert_eq!(
        resp_json.status, "success",
        "get-object (range start) succeeded"
    );
    assert_eq!(resp_json.data.content_length, 4);
    assert_eq!(
        resp_json.data.initial_chunk.as_ref().unwrap().bytes,
        b"abcd".to_vec()
    );

    Ok(())
}

/// Helper function that tests operations on objects that require chunked responses:
///
/// - put_object
/// - get_object (w/ range specified)
///
/// In order to perform these tests, this function assumes you have a:
///
/// - running httpserver provider, accessible at `base_url`
/// - running blobstore-http-smithy actor, connected to that httpserver provider
async fn test_ops_object_chunking(
    base_url: impl AsRef<str>,
    http_client: &reqwest::Client,
) -> Result<()> {
    let base_url = base_url.as_ref();
    let num = rand::random::<u64>();
    let container = format!("test.object-chunking.{}", num);

    // Create a container to house the test object(s)
    let resp_json: ResponseEnvelope<Option<()>> = http_client
        .post(format!("{base_url}/create-container"))
        .body(serde_json::to_string(&json!({"name": container }))?)
        .send()
        .await
        .context("failed to perform POST /create-container")?
        .json()
        .await
        .context("failed to read /create-container response body as json")?;
    assert_eq!(resp_json.status, "success", "create-container succeeded");

    // Insert objects of various sizes
    for count_bytes in [4, 40, 400, 4000].iter() {
        let object = format!("obj_{}", (count_bytes * 25));
        let object_bytes = b"abcdefghijklmnopqrstuvwxy".repeat(*count_bytes);

        // Put the object bytes
        let resp_json: ResponseEnvelope<PutObjectResponse> = http_client
            .post(format!("{base_url}/put-object/{container}/{object}"))
            .body(object_bytes)
            .send()
            .await
            .context("failed to perform POST /put-object")?
            .json()
            .await
            .context("failed to read /put-object response body as json")?;
        assert_eq!(resp_json.status, "success", "put-object succeeded");
    }

    // Ensure that the initial chunk is only 300 bytes
    let resp_json: ResponseEnvelope<GetObjectResponse> = http_client
        .post(format!("{base_url}/get-object"))
        .body(serde_json::to_string(&json!({
            "containerId": container,
            "objectId": "obj_1000",
        }))?)
        .send()
        .await
        .context("failed to perform POST /get-object")?
        .json()
        .await
        .context("failed to read /get-object response body as json")?;
    assert_eq!(
        resp_json.status, "success",
        "get-object (range start) succeeded"
    );
    let initial_chunk = resp_json
        .data
        .initial_chunk
        .context("failed to get initial chunk")?;
    assert_eq!(initial_chunk.bytes.len(), 300);

    Ok(())
}

#[derive(Debug, PartialEq, Eq, Deserialize)]
struct ResponseEnvelope<T> {
    pub status: String,
    pub data: T,
}

/// A copy of the type defined in WIT that represents the result of an
/// operation performed by the blobstore
#[derive(Debug, Deserialize, PartialEq, Eq)]
struct OperationResult {
    key: String,
    error: Option<String>,
    success: bool,
}

/// A copy of the type defined in WIT (normally bindgen-generated)
#[derive(Debug, Deserialize, PartialEq, Eq)]
struct Timestamp {
    sec: u64,
    nsec: u32,
}

/// A copy of the type defined in WIT (normally bindgen-generated)
#[derive(Debug, Deserialize, PartialEq, Eq)]
#[serde(rename_all = "camelCase")]
struct ObjectMetadata {
    container_id: String,
    object_id: String,
    content_length: u64,
    last_modified: Option<Timestamp>,
    content_type: Option<String>,
    content_encoding: Option<String>,
}

/// Copy of [`wasmcloud_interface_blobstore::PutObjectResponse`]
#[derive(Debug, Deserialize, PartialEq, Eq)]
#[serde(rename_all = "camelCase")]
pub struct PutObjectResponse {
    stream_id: Option<String>,
}

/// Copy of [`wasmcloud_interface_blobstore::ListObjectsResponse`]
#[derive(Debug, Deserialize, PartialEq, Eq)]
#[serde(rename_all = "camelCase")]
pub struct ListObjectsResponse {
    objects: Vec<ObjectMetadata>,
    is_last: bool,
    continuation: Option<String>,
}

/// Copy of [`wasmcloud_interface_blobstore::GetObjectResponse`]
#[derive(Debug, Deserialize, PartialEq, Eq)]
#[serde(rename_all = "camelCase")]
pub struct GetObjectResponse {
    success: bool,
    error: Option<String>,
    initial_chunk: Option<Chunk>,
    content_length: u64,
    content_type: Option<String>,
    content_encoding: Option<String>,
}

/// Copy of [`wasmcloud_interface_blobstore::Chunk`]
#[derive(Debug, Deserialize, PartialEq, Eq)]
#[serde(rename_all = "camelCase")]
pub struct Chunk {
    object_id: String,
    container_id: String,
    bytes: Vec<u8>,
    offset: u64,
    is_last: bool,
}
