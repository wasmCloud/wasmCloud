//! Utilities for managing on-lattice configuration
use std::collections::HashMap;

use anyhow::{anyhow, Result};
use wasmcloud_control_interface::{Client as WasmCloudCtlClient, CtlResponse};

/// Put a configuration value, ensuring that the put succeeded
pub async fn assert_config_put(
    client: impl Into<&WasmCloudCtlClient>,
    name: impl AsRef<str>,
    config: HashMap<String, String>,
) -> Result<CtlResponse<()>> {
    let client = client.into();
    let name = name.as_ref();
    client
        .put_config(name, config)
        .await
        .map_err(|e| anyhow!(e).context("failed to put config"))
}
