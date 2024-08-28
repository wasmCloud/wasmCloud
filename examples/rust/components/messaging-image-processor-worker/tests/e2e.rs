//! NOTE: these tests in this module are *very* finicky, due to lack of integration
//! with a released newer version wasmcloud-test-utils
//!  
//! They currently require:
//! - wasmcloud to be installed locally (not dockerized)
//! - *one* instance to be running (spawned by the test, see `TestEnv`)
//!
//! Upon unexpected/early failure of a test, you will almost certainly have left over instances, and may
//! need to run `wash down --all` and/or `wash app delete <....>` manually.
//!  

use anyhow::{bail, Context as _, Result};
use bytes::Bytes;
use serde_json::json;
use tokio::time::{Duration, Instant};

mod common;
use common::{setup_test_env, TestEnv, TEST_SUBSCRIPTION_NAME};

mod bindings {
    wit_bindgen::generate!({ generate_all });
}

/// Re-use processing code
#[path = "../src/processing.rs"]
mod processing;

use processing::{
    BlobstorePath, ImageOperation, ImagePath, ImageProcessingRequest, JobMessage,
    DEFAULT_IMAGE_BYTES,
};

const LOG_CONTEXT: &str = "image-processor-worker";
const WORKER_ID: &str = "rust-component-worker";

/// Task group ID
const GROUP_ID: &str = "test";

/// Data sent back by the API after a job submission
#[derive(Debug, PartialEq, Eq, ::serde::Deserialize)]
struct JobSubmitApiResponse {
    job_id: String,
}

#[derive(Debug, PartialEq, Eq, ::serde::Deserialize)]
struct ApiResponse<T> {
    status: String,
    data: Option<T>,
    error: Option<ApiError>,
}

#[derive(Debug, PartialEq, Eq, ::serde::Deserialize)]
struct ApiError {
    code: String,
    message: String,
}

/// Ensure that a roundtrip of the component works
///
/// NOTE: this test is *not* robust to running with multiple other tests,
/// unlike the setup in wash-cli and related tests.
///
/// This test (and the setup required) expects to be run as the only test, and the only one performing `wash` commands.
/// Early termination, unexpected errors, etc will likely leave leftover wasmcloud/wadm instances and require running `wash down`.
///
/// See [`setup_test_env`] for more information.
///
#[tokio::test(flavor = "multi_thread")]
async fn test_roundtrip() -> Result<()> {
    let env = setup_test_env().await?;

    trigger_task_mgr_migration(&env)
        .await
        .context("failed to trigger task component migration")?;

    // Submit an image processing job (w/ the right image path)
    let task_json = create_task(
        &env,
        GROUP_ID,
        &serde_json::to_value(&ImageProcessingRequest {
            source: ImagePath::DefaultImage,
            destination: ImagePath::Blobstore {
                path: BlobstorePath {
                    bucket: "output".into(),
                    key: "complete".into(),
                },
            },
            image_format: None,
            operations: vec![ImageOperation::Grayscale],
            image_data: None,
        })
        .context("failed to convert procesisng request to value")?,
    )
    .await?;
    let task_id = task_json["id"]
        .as_str()
        .context("task ID was not a string")?;

    // Send a job message on NATS for the messaging component to pick up
    env.nats_client
        .publish(
            TEST_SUBSCRIPTION_NAME,
            Bytes::from(
                serde_json::to_vec(&JobMessage {
                    task_id: task_id.into(),
                })
                .context("failed to serialize job message")?,
            ),
        )
        .await
        .context("publishing message failed")?;

    // After the job is done we expect a file to be written out to the folder use by blobstore-fs
    // with the image bytes
    let output_path = std::env::temp_dir().join(format!("/test-messaging-processor/{task_id}"));

    // Wait until the task is marked as finished
    let start = Instant::now();
    let file_bytes = loop {
        tokio::time::sleep(tokio::time::Duration::from_secs(3)).await;
        let task_json = get_task_by_id(&env, task_id)
            .await
            .context("failed to retrieve task by ID")?;
        // If the job is complete, then read the file data from disk
        if task_json["status"] == "completed" {
            break tokio::fs::read(&output_path)
                .await
                .with_context(|| format!("missing output file @ [{}]", output_path.display()))?;
        }
        if Instant::now().duration_since(start) > Duration::from_secs(30) {
            bail!(
                "task not finished, still in status [{}]",
                task_json["status"],
            );
        }
    };

    assert!(
        DEFAULT_IMAGE_BYTES != file_bytes,
        "bytes in the default image matched the original image"
    );
    Ok(())
}

/// Helper method for creating a new task
async fn create_task(
    TestEnv {
        http_client,
        base_url,
        ..
    }: &TestEnv,
    group_id: &str,
    task_data: &serde_json::Value,
) -> Result<serde_json::Value> {
    // Trigger the DB migration via hitting the admin API
    let resp = http_client
        .post(format!("{base_url}/api/v1/tasks"))
        .json(&json!({
            "group_id": group_id,
            "task_data": task_data,
        }))
        .send()
        .await
        .context("failed to create task")?;
    assert!(
        resp.status().is_success(),
        "create task response was not success"
    );
    let body = resp
        .json::<ApiResponse<serde_json::Value>>()
        .await
        .context("failed to parse json body from task creation")?;
    assert_eq!(body.status, "success", "creating task failed");
    body.data.context("missing created task in API response")
}

/// Helper method for retrieving a task by ID
async fn get_task_by_id(
    TestEnv {
        http_client,
        base_url,
        ..
    }: &TestEnv,
    task_id: &str,
) -> Result<serde_json::Value> {
    // Trigger the DB migration via hitting the admin API
    let resp = http_client
        .get(format!("{base_url}/api/v1/tasks/{task_id}"))
        .send()
        .await
        .with_context(|| format!("failed to get task with ID [{task_id}]"))?;
    assert!(
        resp.status().is_success(),
        "get task by ID response was not success"
    );
    let body = resp
        .json::<ApiResponse<serde_json::Value>>()
        .await
        .context("failed to parse JSON object from get task response")?;
    assert_eq!(body.status, "success", "retrieving single task failed");
    body.data.context("missing retrieved task in API response")
}

/// Helper method for triggering the database migration of the task manager
async fn trigger_task_mgr_migration(
    TestEnv {
        http_client,
        base_url,
        ..
    }: &TestEnv,
) -> Result<()> {
    // Trigger the DB migration via hitting the admin API
    let resp = http_client
        .post(format!("{base_url}/admin/v1/db/migrate"))
        .send()
        .await
        .context("failed to perform migrate request")?;
    assert!(resp.status().is_success(), "failed to migrate database");
    let body = resp
        .json::<ApiResponse<serde_json::Value>>()
        .await
        .context("failed to parse json body")?;
    assert!(
        body.status == "success",
        "migration did not return success status"
    );
    Ok(())
}
