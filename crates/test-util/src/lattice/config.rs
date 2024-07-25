//! Utilities for managing on-lattice configuration
use std::collections::HashMap;

use anyhow::{anyhow, ensure, Result};
use wasmcloud_control_interface::{Client as WasmCloudCtlClient, CtlResponse};

/// Put a configuration value, ensuring that the put succeeded
pub async fn assert_config_put(
    client: impl Into<&WasmCloudCtlClient>,
    name: impl AsRef<str>,
    config: impl Into<HashMap<String, String>>,
) -> Result<()> {
    let client = client.into();
    let name = name.as_ref();
    let CtlResponse { success, .. } = client
        .put_config(name, config)
        .await
        .map_err(|e| anyhow!(e).context("failed to put config"))?;

    ensure!(success);

    Ok(())
}

pub async fn assert_put_secret_reference(
    client: impl Into<&WasmCloudCtlClient>,
    name: impl AsRef<str>,
    key: &str,
    backend: &str,
    version: Option<String>,
) -> Result<()> {
    let mut config = HashMap::from([
        ("backend".to_string(), backend.to_string()),
        ("key".to_string(), key.to_string()),
        (
            "policy".to_string(),
            serde_json::json!({"type": "policy.secrets.wasmcloud.dev/v1alpha1", "properties": {"configuration": "value"}}).to_string(),
        ),
    ]);
    if let Some(version) = version {
        config.insert("version".to_string(), version.to_string());
    }

    assert_config_put(client, format!("SECRET_{}", name.as_ref()), config).await
}
