//! Utilities for managing on-lattice configuration
use std::collections::HashMap;

use anyhow::{anyhow, Result};

/// Put a configuration value, ensuring that the put succeeded
pub async fn assert_config_put(
    client: impl AsRef<wasmcloud_control_interface::Client>,
    name: impl AsRef<str>,
    config: HashMap<String, String>,
) -> Result<()> {
    let client = client.as_ref();
    let name = name.as_ref();
    client
        .put_config(name, config)
        .await
        .map(|_| ())
        .map_err(|e| anyhow!(e).context("failed to put config"))
}
