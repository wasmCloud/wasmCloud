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
