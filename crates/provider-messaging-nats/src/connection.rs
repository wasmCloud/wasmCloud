use std::collections::HashMap;

use tracing::warn;
use wasmcloud_core::messaging::{
    ConnectionConfig, CONFIG_NATS_CLIENT_JWT, CONFIG_NATS_CLIENT_SEED,
};
use wasmcloud_provider_sdk::{core::secrets::SecretValue, LinkConfig};

/// Create a [`ConnectionConfig`] from a given [`LinkConfig`]
pub fn from_link_config(
    LinkConfig {
        secrets, config, ..
    }: &LinkConfig,
) -> anyhow::Result<ConnectionConfig> {
    let mut map = HashMap::clone(config);

    if let Some(jwt) = secrets
        .get(CONFIG_NATS_CLIENT_JWT)
        .and_then(SecretValue::as_string)
        .or_else(|| {
            warn!("secret value [{CONFIG_NATS_CLIENT_JWT}] was found not found in secrets. Prefer using secrets for sensitive values.");
            config.get(CONFIG_NATS_CLIENT_JWT).map(String::as_str)
        })
    {
        map.insert(CONFIG_NATS_CLIENT_JWT.into(), jwt.to_string());
    }

    if let Some(seed) = secrets
        .get(CONFIG_NATS_CLIENT_SEED)
        .and_then(SecretValue::as_string)
        .or_else(|| {
            warn!("secret value [{CONFIG_NATS_CLIENT_SEED}] was found not found in secrets. Prefer using secrets for sensitive values.");
            config.get(CONFIG_NATS_CLIENT_SEED).map(String::as_str)
        })
    {
        map.insert(CONFIG_NATS_CLIENT_SEED.into(), seed.to_string());
    }

    ConnectionConfig::from_map(&map)
}
