use std::time::Duration;

use anyhow::{Context, Result};
use serial_test::serial;
use tokio::process::Command;
use wadm_types::api::StatusType;
use wash::lib::app::validate_manifest_file;
use wash::lib::cli::get::parse_watch_interval;
use wash::lib::cli::output::{AppDeployCommandOutput, AppValidateOutput};

mod common;
use common::TestWashInstance;

// Using tinygo hello world example manifest
const TINYGO_HELLO_WORLD_MANIFEST_CONTENT: &str = r#"
apiVersion: core.oam.dev/v1beta1
kind: Application
metadata:
  name: tinygo-hello-world
  annotations:
    description: 'HTTP hello world demo in Golang (TinyGo), using the WebAssembly Component Model and WebAssembly Interfaces Types (WIT)'
    wasmcloud.dev/authors: wasmCloud team
    wasmcloud.dev/source-url: https://github.com/wasmCloud/wasmCloud/blob/main/examples/golang/components/http-hello-world/wadm.yaml
    wasmcloud.dev/readme-md-url: https://github.com/wasmCloud/wasmCloud/blob/main/examples/golang/components/http-hello-world/README.md
    wasmcloud.dev/homepage: https://github.com/wasmCloud/wasmCloud/tree/main/examples/golang/components/http-hello-world
    wasmcloud.dev/categories: |
      http,outgoing-http,http-server,tinygo,golang,example
spec:
  components:
    - name: http-component
      type: component
      properties:
        image: file://./build/http_hello_world_s.wasm
      traits:
        - type: spreadscaler
          properties:
            instances: 1
    - name: httpserver
      type: capability
      properties:
        image: ghcr.io/wasmcloud/http-server:0.22.0
      traits:
        - type: link
          properties:
            target: http-component
            namespace: wasi
            package: http
            interfaces: [incoming-handler]
            source_config:
              - name: default-http
                properties:
                  address: 127.0.0.1:8080
    "#;

/// Ensure a simple WADM manifest passes validation
#[tokio::test]
async fn app_validate_simple() -> Result<()> {
    let pass = "./tests/fixtures/wadm/simple.wadm.yaml";
    tokio::fs::try_exists(pass).await?;
    let output = Command::new(env!("CARGO_BIN_EXE_wash"))
        .args([
            "app",
            "validate",
            "./tests/fixtures/wadm/manifests/simple.wadm.yaml",
            "--output",
            "json",
        ])
        .kill_on_drop(true)
        .output()
        .await
        .context("failed to execute wash app validate")?;

    let cmd_output: AppValidateOutput =
        serde_json::from_slice(&output.stdout).context("failed to build JSON from output")?;
    assert!(cmd_output.valid, "valid output");
    assert!(cmd_output.errors.is_empty(), "no errors");
    assert!(cmd_output.warnings.is_empty(), "no warnings");

    Ok(())
}

#[tokio::test]
#[cfg_attr(not(can_reach_ghcr_io), ignore = "ghcr.io is not reachable")]
async fn test_validate_complete_wadm_manifest() {
    let test_dir = std::env::temp_dir().join("validate_complete_wadm_manifest");
    let manifest_file_path = test_dir.join("wadm_manifest.yaml");

    // Create the test directory
    tokio::fs::create_dir_all(&test_dir)
        .await
        .expect("Failed to create test directory");

    // Write out the manifest
    tokio::fs::write(&manifest_file_path, TINYGO_HELLO_WORLD_MANIFEST_CONTENT)
        .await
        .expect("Failed to write test manifest file");

    let result = validate_manifest_file(&manifest_file_path, true).await;
    assert!(result.is_ok(), "Validation failed: {:?}", result.err());

    // Clean up test directory
    tokio::fs::remove_dir_all(&test_dir)
        .await
        .expect("Failed to clean up test directory");
}

#[test]
fn test_parse_watch_interval_milliseconds() {
    // Test parsing normal millisecond input
    let result = parse_watch_interval("1500").unwrap();
    assert_eq!(result, Duration::from_millis(1500));
}

#[test]
fn test_parse_watch_interval_humantime_seconds() {
    // Test parsing humantime input (5s)
    let result = parse_watch_interval("5s").unwrap();
    assert_eq!(result, Duration::from_secs(5));
}

#[test]
fn test_parse_watch_interval_invalid_input() {
    // Test invalid input
    let result = parse_watch_interval("invalid");
    assert!(result.is_err());
    assert_eq!(
            result.unwrap_err(),
            "Invalid duration: 'invalid'. Expected a duration like '5s', '1m', '100ms', or milliseconds as an integer."
        );
}

/// Ensure that `wash app undeploy --all` and `wash app --delete-undeployed` work
#[tokio::test]
#[serial]
async fn test_undeploy_all_and_delete_undeployed() -> Result<()> {
    let instance = TestWashInstance::create().await?;
    // Deploy the application
    let AppDeployCommandOutput {
        success,
        deployed,
        model_name,
        model_version,
    } = instance
        .deploy_app("./tests/fixtures/wadm/manifests/simple.wadm.yaml")
        .await?;
    assert!(success && deployed);
    assert_eq!(model_name, "sample");
    assert_eq!(model_version, "v0.0.1");

    // Wait until the app is deployed via wash app get
    tokio::time::timeout(Duration::from_secs(30), async {
        loop {
            if instance.list_apps().await.is_ok_and(|output| {
                output.applications.iter().any(|a| {
                    a.name == "sample" && a.detailed_status.info.status_type == StatusType::Deployed
                })
            }) {
                break;
            }
            tokio::time::sleep(tokio::time::Duration::from_millis(250)).await;
        }
    })
    .await
    .context("timed out waiting for app to be deployed")?;

    // Perform an undeploy all
    instance.undeploy_all_apps().await?;

    // Wait until the app is deployed via wash app get
    tokio::time::timeout(Duration::from_secs(30), async {
        loop {
            if instance.list_apps().await.is_ok_and(|output| {
                output.applications.iter().any(|a| {
                    a.name == "sample"
                        && a.detailed_status.info.status_type == StatusType::Undeployed
                })
            }) {
                break;
            }
            tokio::time::sleep(tokio::time::Duration::from_millis(250)).await;
        }
    })
    .await
    .context("timed out waiting for app to be undeployed")?;

    // Perform delete all
    instance.delete_all_undeployed_apps().await?;

    // Wait until the app is deployed via wash app get
    tokio::time::timeout(Duration::from_secs(30), async {
        loop {
            if instance
                .list_apps()
                .await
                .is_ok_and(|output| output.applications.is_empty())
            {
                break;
            }
            tokio::time::sleep(tokio::time::Duration::from_millis(250)).await;
        }
    })
    .await
    .context("timed out waiting for app to be deleted")?;

    Ok(())
}

/// Ensure that `wash app undeploy --all` and `wash app --delete-undeployed` work
// Should break when we deprecate the `wash app list` command
#[tokio::test]
#[serial]
async fn test_app_without_name_is_same_as_wash_app_list() -> Result<()> {
    let instance = TestWashInstance::create().await?;
    // Deploy the application
    let AppDeployCommandOutput {
        success,
        deployed,
        model_name,
        model_version,
    } = instance
        .deploy_app("./tests/fixtures/wadm/manifests/simple.wadm.yaml")
        .await?;
    assert!(success && deployed);
    assert_eq!(model_name, "sample");
    assert_eq!(model_version, "v0.0.1");

    // Wait until the app is deployed via wash app get
    tokio::time::timeout(Duration::from_secs(30), async {
        loop {
            if instance.list_apps().await.is_ok_and(|output| {
                output.applications.iter().any(|a| {
                    a.name == "sample" && a.detailed_status.info.status_type == StatusType::Deployed
                })
            }) {
                break;
            }
            tokio::time::sleep(tokio::time::Duration::from_millis(250)).await;
        }
    })
    .await
    .context("timed out waiting for app to be deployed")?;

    // Get all the apps with wash app get without specifying the app name
    let listed_apps = instance.list_apps().await?;
    assert_eq!(listed_apps.applications.len(), 1);

    let only_app_from_list = listed_apps.applications.first();
    assert!(only_app_from_list.is_some());
    let only_app_from_list = only_app_from_list.unwrap();

    // Get all apps with wash app list
    let listed_apps_with_get = instance.get_apps().await?;
    assert_eq!(listed_apps_with_get.applications.len(), 1);

    let app_with_get = listed_apps_with_get.applications.first();
    assert!(app_with_get.is_some());
    let app_with_get = app_with_get.unwrap();

    // The two response the same (maybe &ModelSummary should impl Eq, PartialEq)
    assert_eq!(only_app_from_list.name, app_with_get.name);
    assert_eq!(only_app_from_list.description, app_with_get.description);
    assert_eq!(
        only_app_from_list.detailed_status,
        app_with_get.detailed_status
    );
    assert_eq!(only_app_from_list.version, app_with_get.version);

    Ok(())
}
