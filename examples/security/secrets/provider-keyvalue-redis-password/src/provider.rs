use std::sync::Arc;

use anyhow::{bail, Context as _};
use redis::aio::ConnectionManager;
use redis::AsyncCommands;
use tokio::sync::RwLock;
use tracing::info;
use tracing::warn;
use wasmcloud_provider_sdk::core::secrets::SecretValue;
use wasmcloud_provider_sdk::core::HostData;
use wasmcloud_provider_sdk::serve_provider_exports;
use wasmcloud_provider_sdk::Provider;
use wasmcloud_provider_sdk::{load_host_data, run_provider, Context};

mod bindings {
    wit_bindgen_wrpc::generate!({
        with: {
            "wrpc:keyvalue/atomics@0.2.0-draft": generate,
            "wrpc:keyvalue/store@0.2.0-draft": generate,
        }
    });
}
use bindings::exports::wrpc::keyvalue;

const DEFAULT_REDIS_URL: &str = "127.0.0.1:6379";

#[derive(Clone)]
pub struct SecretsExampleProvider {
    default_connection: Arc<RwLock<ConnectionManager>>,
}

impl SecretsExampleProvider {
    pub async fn run() -> anyhow::Result<()> {
        let HostData {
            secrets, config, ..
        } = load_host_data().context("failed to load host data")?;
        let SecretValue::String(password) = secrets
            .get("redis_password")
            .context(format!("redis_password secret not found: {:?}", secrets))?
        else {
            bail!("password secret not a string")
        };
        let url = config
            .get("url")
            .cloned()
            .unwrap_or_else(|| DEFAULT_REDIS_URL.to_string());
        let conn_manager = match redis::Client::open(format!("redis://:{password}@{url}")) {
            Ok(client) => match client.get_connection_manager().await {
                Ok(conn) => {
                    info!(url, "connected to redis with password");
                    conn
                }
                Err(err) => {
                    warn!(
                        url,
                        ?err,
                        "Could not create Redis connection manager, keyvalue operations will fail",
                    );
                    bail!("failed to create redis connection manager");
                }
            },
            Err(err) => {
                warn!(
                    ?err,
                    "Could not create Redis client, keyvalue operations will fail",
                );
                bail!("failed to create redis client");
            }
        };
        let provider = SecretsExampleProvider {
            default_connection: Arc::new(RwLock::new(conn_manager)),
        };
        let shutdown = run_provider(provider.clone(), "secrets-example-provider")
            .await
            .context("failed to run provider")?;
        let connection = wasmcloud_provider_sdk::get_connection();
        serve_provider_exports(
            &connection.get_wrpc_client(connection.provider_key()),
            provider,
            shutdown,
            bindings::serve,
        )
        .await
    }
}

impl keyvalue::atomics::Handler<Option<Context>> for SecretsExampleProvider {
    async fn increment(
        &self,
        _ctx: Option<Context>,
        _bucket: String,
        key: String,
        delta: u64,
    ) -> anyhow::Result<Result<u64, keyvalue::atomics::Error>> {
        Ok(self
            .default_connection
            .write()
            .await
            .incr(key, delta)
            .await
            .map_err(|e| keyvalue::atomics::Error::Other(format!("redis error: {:?}", e))))
    }
}

impl Provider for SecretsExampleProvider {}
