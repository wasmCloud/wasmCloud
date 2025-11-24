use std::collections::HashMap;

use tracing::warn;
use wasmcloud_core::messaging::{
    ConnectionConfig, CONFIG_NATS_CLIENT_JWT, CONFIG_NATS_CLIENT_SEED,
};
use wasmcloud_provider_sdk::{core::secrets::SecretValue, types::InterfaceConfig};

/// Create a [`ConnectionConfig`] from a given [`LinkConfig`]
pub fn from_link_config(link_config: InterfaceConfig) -> anyhow::Result<ConnectionConfig> {
    // Convert config Vec to HashMap
    let mut map: HashMap<String, String> = link_config.config.iter().cloned().collect();
    let secrets = &link_config.secrets;

    let get_secret = |k: &str| -> Option<String> {
        secrets
            .as_ref()
            .and_then(|s| s.iter().find(|(key, _)| key == k))
            .and_then(|(_, v)| {
                let secret: SecretValue = v.into();
                secret.as_string().map(String::from)
            })
    };

    if let Some(jwt) = get_secret(CONFIG_NATS_CLIENT_JWT)
        .or_else(|| {
            warn!("secret value [{CONFIG_NATS_CLIENT_JWT}] was not found in secrets. Prefer using secrets for sensitive values.");
            map.get(CONFIG_NATS_CLIENT_JWT).cloned()
        })
    {
        map.insert(CONFIG_NATS_CLIENT_JWT.into(), jwt);
    }

    if let Some(seed) = get_secret(CONFIG_NATS_CLIENT_SEED)
        .or_else(|| {
            warn!("secret value [{CONFIG_NATS_CLIENT_SEED}] was not found in secrets. Prefer using secrets for sensitive values.");
            map.get(CONFIG_NATS_CLIENT_SEED).cloned()
        })
    {
        map.insert(CONFIG_NATS_CLIENT_SEED.into(), seed);
    }

    ConnectionConfig::from_map(&map)
}
