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

use anyhow::{Context as _, Result};
use serde_json::json;

mod common;
use common::{setup_test_env, TestEnv};

mod bindings {
    wit_bindgen::generate!({ generate_all });
}

use crate::bindings::wasmcloud::task_manager::types::{LeaseId, Task};

const GROUP_ID: &str = "test-group";
const WORKER_ID: &str = "test-worker";

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

/// Re-use of serde machinations
#[path = "../src/serde.rs"]
mod serde;

/// Ensure that sending HTTP requests to the
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

    // Migrate the database
    trigger_db_migration(&env).await?;

    // Get a list of existing tasks
    let existing_tasks = get_tasks(&env).await?;
    assert!(existing_tasks.is_empty(), "unexpected tasks were present");

    // Add a new task
    let task_data = json!({
        "task_type": "example",
        "data": true,
    });
    let task = create_task(&env, GROUP_ID, &task_data).await?;
    assert_eq!(task.group_id, GROUP_ID);
    let task_data_json = task
        .data_json
        .context("missing task data json from saved task")?;
    assert!(
        serde_json::from_str::<serde_json::Value>(&task_data_json).is_ok_and(|v| v == task_data)
    );

    // Retrieve the list of tasks
    let existing_tasks = get_tasks(&env).await?;
    assert_eq!(
        existing_tasks.len(),
        1,
        "unexpected count of existing tasks"
    );
    let Task { id, leased_at, .. } = existing_tasks.first().context("failed to get since task")?;
    assert!(!id.is_empty(), "task ID is unexpectedly empty");
    assert!(leased_at.is_none(), "task should not be leased");

    // Lease task, getting a lease_id in return
    let lease_id = lease_task(&env, id, WORKER_ID).await?;
    assert!(!lease_id.is_empty(), "lease ID was unexpectedly empty");

    // Retrieve the task we just leased, by ID
    let retrieved_task = get_task_by_id(&env, id).await?;
    assert_eq!(
        id, &retrieved_task.id,
        "retrieved task doesn't match created"
    );
    assert!(retrieved_task.leased_at.is_some(), "leased at is filled");
    // The backend keeps the lease ID a secret to others, but we can ensure we were selected
    assert_eq!(
        retrieved_task.lease_worker_id,
        Some(WORKER_ID.into()),
        "lease is filled by unexpected worker"
    );

    // Release the task, to possibly another workers
    release_task(&env, id, &lease_id, WORKER_ID).await?;

    // Check that the task was properly released
    let retrieved_task = get_task_by_id(&env, id).await?;
    assert_eq!(
        id, &retrieved_task.id,
        "retrieved task doesn't match created"
    );
    assert!(
        retrieved_task.leased_at.is_none(),
        "the lease start is reset"
    );
    assert!(
        retrieved_task.lease_worker_id.is_none(),
        "lease is not released (has lease_worker_id)"
    );

    // re-lease task
    let new_lease_id = lease_task(&env, id, WORKER_ID).await?;
    assert!(!lease_id.is_empty(), "lease ID was unexpectedly empty");
    assert!(
        lease_id != new_lease_id,
        "new lease ID is not the same as previous"
    );

    // mark the task completed
    mark_task_completed(&env, id, &lease_id, WORKER_ID).await?;
    let retrieved_task = get_task_by_id(&env, id).await?;
    assert_eq!(
        id, &retrieved_task.id,
        "retrieved task doesn't match created"
    );
    assert!(
        retrieved_task.leased_at.is_some(),
        "the lease start was unexpectedly reset after completion"
    );
    assert!(
        retrieved_task.completed_at.is_some(),
        "completed at is filled in"
    );

    // Add a new task which we will fail
    let fail_task = create_task(
        &env,
        GROUP_ID,
        &json!({
            "task_type": "example-fail",
            "data": true,
        }),
    )
    .await?;
    let fail_task_lease_id = lease_task(&env, &fail_task.id, WORKER_ID).await?;

    // Mark the new task failed
    mark_task_failed(
        &env,
        &fail_task.id,
        &fail_task_lease_id,
        WORKER_ID,
        "test-failure-reason",
    )
    .await?;
    let retrieved_fail_task = get_task_by_id(&env, &fail_task.id).await?;
    assert_eq!(
        fail_task.id, retrieved_fail_task.id,
        "retrieved task doesn't match newly created"
    );
    assert!(
        retrieved_fail_task.leased_at.is_some(),
        "the lease start was unexpectedly reset after completion"
    );
    assert!(
        retrieved_fail_task.last_failed_at.is_some(),
        "failed_at is filled in"
    );

    Ok(())
}

/// Helper method for triggering the database migration of a given test setup
async fn trigger_db_migration(
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
    assert!(resp.status().is_success(), "failed to retrieve tasks list");
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

/// Helper method for retrieving tasks
async fn get_tasks(
    TestEnv {
        http_client,
        base_url,
        ..
    }: &TestEnv,
) -> Result<Vec<Task>> {
    // Trigger the DB migration via hitting the admin API
    let resp = http_client
        .get(format!("{base_url}/api/v1/tasks"))
        .send()
        .await
        .context("failed to retrieve tasks")?;
    assert!(
        resp.status().is_success(),
        "failed to retrieve tasks from db"
    );
    let body = resp
        .json::<ApiResponse<Vec<Task>>>()
        .await
        .context("failed to parse json body")?;
    assert!(body.status == "success", "retrieving tasks failed");
    body.data.context("missing tasks in API response")
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
) -> Result<Task> {
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
        .json::<ApiResponse<Task>>()
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
) -> Result<Task> {
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
        .json::<ApiResponse<Task>>()
        .await
        .context("failed to parse json body from get task")?;
    assert_eq!(body.status, "success", "retrieving single task failed");
    body.data.context("missing retrieved task in API response")
}

/// Helper method for retrieving a task by ID
async fn lease_task(
    TestEnv {
        http_client,
        base_url,
        ..
    }: &TestEnv,
    task_id: &str,
    worker_id: &str,
) -> Result<LeaseId> {
    // Trigger the DB migration via hitting the admin API
    let resp = http_client
        .post(format!("{base_url}/api/v1/tasks/{task_id}/lease"))
        .json(&json!({ "worker_id": worker_id }))
        .send()
        .await
        .with_context(|| format!("failed to lease task with ID [{task_id}]"))?;
    assert!(
        resp.status().is_success(),
        "lease task response was not success"
    );
    let body = resp
        .json::<ApiResponse<LeaseId>>()
        .await
        .context("failed to parse lease ID from lease task")?;
    assert_eq!(body.status, "success", "leasing task failed");
    body.data.context("missing leased task ID in API response")
}

/// Helper method for retrieving a task by ID
async fn release_task(
    TestEnv {
        http_client,
        base_url,
        ..
    }: &TestEnv,
    task_id: &str,
    lease_id: &str,
    worker_id: &str,
) -> Result<()> {
    // Trigger the DB migration via hitting the admin API
    let resp = http_client
        .post(format!("{base_url}/api/v1/tasks/{task_id}/release"))
        .json(&json!({
            "lease_id": lease_id,
            "worker_id": worker_id,
        }))
        .send()
        .await
        .with_context(|| format!("failed to release task with ID [{task_id}]"))?;
    assert!(
        resp.status().is_success(),
        "release task response was not success"
    );
    let body = resp
        .json::<ApiResponse<()>>()
        .await
        .context("failed to parse json body from releaase task")?;
    assert_eq!(body.status, "success", "releasing task failed");
    assert!(
        body.data.is_none(),
        "data was unexpectedly present in release task response"
    );
    Ok(())
}

/// Helper method for marking a task complete
async fn mark_task_completed(
    TestEnv {
        http_client,
        base_url,
        ..
    }: &TestEnv,
    task_id: &str,
    lease_id: &str,
    worker_id: &str,
) -> Result<()> {
    // Trigger the DB migration via hitting the admin API
    let resp = http_client
        .post(format!("{base_url}/api/v1/tasks/{task_id}/complete"))
        .json(&json!({
            "lease_id": lease_id,
            "worker_id": worker_id,
        }))
        .send()
        .await
        .with_context(|| format!("failed to complete task with ID [{task_id}]"))?;
    assert!(
        resp.status().is_success(),
        "task complete response was not success"
    );
    let body = resp
        .json::<ApiResponse<()>>()
        .await
        .context("failed to parse json body from task complete")?;
    assert!(
        body.data.is_none(),
        "data was unexpectedly present in release task response"
    );
    Ok(())
}

/// Helper method for marking a task failed
async fn mark_task_failed(
    TestEnv {
        http_client,
        base_url,
        ..
    }: &TestEnv,
    task_id: &str,
    lease_id: &str,
    worker_id: &str,
    reason: &str,
) -> Result<()> {
    // Trigger the DB migration via hitting the admin API
    let resp = http_client
        .post(format!("{base_url}/api/v1/tasks/{task_id}/fail"))
        .json(&json!({
            "lease_id": lease_id,
            "worker_id": worker_id,
            "reason": reason,
        }))
        .send()
        .await
        .with_context(|| format!("failed to fail task with ID [{task_id}]"))?;
    assert!(
        resp.status().is_success(),
        "task fail response was not success"
    );
    let body = resp
        .json::<ApiResponse<()>>()
        .await
        .context("failed to parse json body from task fail")?;
    assert!(
        body.data.is_none(),
        "data was unexpectedly present in release task response"
    );
    Ok(())
}
