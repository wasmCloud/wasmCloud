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
    field: Option<String>,
    version: Option<String>,
    properties: HashMap<String, String>,
) -> Result<()> {
    let secret_config = wasmcloud_secrets_types::SecretConfig::new(
        name.as_ref().to_string(),
        backend.to_string(),
        key.to_string(),
        field,
        version,
        properties.into_iter().map(|(k, v)| (k, v.into())).collect(),
    );

    let config: HashMap<String, String> = secret_config.try_into()?;

    assert_config_put(client, format!("SECRET_{}", name.as_ref()), config).await
}
