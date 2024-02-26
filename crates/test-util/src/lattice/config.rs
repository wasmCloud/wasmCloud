//! Utilities for managing on-lattice configuration
use anyhow::{anyhow, Result};
use wascap::jwt;

/// Put a configuration value, ensuring that the put succeeded
pub async fn assert_config_put(
    client: impl AsRef<wasmcloud_control_interface::Client>,
    actor_claims: impl AsRef<jwt::Claims<jwt::Actor>>,
    key: impl AsRef<str>,
    value: impl Into<Vec<u8>>,
) -> Result<()> {
    let client = client.as_ref();
    let actor_claims = actor_claims.as_ref();
    let key = key.as_ref();
    let value = value.into();
    client
        .put_config(&actor_claims.subject, key, value)
        .await
        .map(|_| ())
        .map_err(|e| anyhow!(e).context("failed to put config"))
}
