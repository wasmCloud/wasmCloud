use anyhow::{Context, Result};
use tokio::process::Command;
use wash_lib::{app::validate_manifest_file, cli::output::AppValidateOutput};

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

    // Using tinygo hello world example manifest
    let manifest_content = r#"
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
    tokio::fs::write(&manifest_file_path, manifest_content)
        .await
        .expect("Failed to write test manifest file");

    let oci_check = true;
    let result = validate_manifest_file(&manifest_file_path, oci_check).await;

    assert!(result.is_ok(), "Validation failed: {:?}", result.err());

    // Clean up test directory
    tokio::fs::remove_dir_all(&test_dir)
        .await
        .expect("Failed to clean up test directory");
}
