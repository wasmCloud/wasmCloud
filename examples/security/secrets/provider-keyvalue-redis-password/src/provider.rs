use std::sync::Arc;

use anyhow::{bail, Context as _};
use exports::wrpc::keyvalue::atomics::Handler;
use redis::AsyncCommands;
use tokio::sync::RwLock;
use tracing::info;
use tracing::warn;
use wasmcloud_provider_sdk::core::secrets::SecretValue;
use wasmcloud_provider_sdk::core::HostData;
use wasmcloud_provider_sdk::Provider;
use wasmcloud_provider_sdk::{load_host_data, run_provider, Context};

use redis::aio::ConnectionManager;

wit_bindgen_wrpc::generate!();

#[derive(Clone)]
/// Your provider struct is where you can store any state or configuration that your provider needs to keep track of.
pub struct SecretsExampleProvider {
    default_connection: Arc<RwLock<ConnectionManager>>,
}

/// This `impl` block is where you can implement additional methods for your provider. We've provided two examples
/// to run and load [`HostData`], and when you have custom logic to implement, you can add it here.
impl SecretsExampleProvider {
    /// Execute the provider, loading [`HostData`] from the host which includes the provider's configuration and
    /// information about the host. Once you use the passed configuration to construct a [`CustomTemplateProvider`],
    /// you can run the provider by calling `run_provider` and then serving the provider's exports on the proper
    /// RPC topics via `wrpc::serve`.
    ///
    /// This step is essentially the same for every provider, and you shouldn't need to modify this function.
    pub async fn run() -> anyhow::Result<()> {
        let HostData {
            secrets, config, ..
        } = load_host_data().context("failed to load host data")?;
        let SecretValue::String(password) = secrets
            .get("redis_password")
            .expect(&format!("redis_password secret not found: {:?}", secrets))
        else {
            bail!("password secret not a string")
        };
        let url = config
            .get("url")
            .cloned()
            .unwrap_or_else(|| "127.0.0.1:6379".to_string());
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
        serve(
            &connection.get_wrpc_client(connection.provider_key()),
            provider,
            shutdown,
        )
        .await
    }
}

impl Handler<Option<Context>> for SecretsExampleProvider {
    async fn increment(
        &self,
        _ctx: Option<Context>,
        _bucket: String,
        key: String,
        delta: u64,
    ) -> anyhow::Result<Result<u64, exports::wrpc::keyvalue::atomics::Error>> {
        Ok(self
            .default_connection
            .write()
            .await
            .incr(key, delta)
            .await
            .map_err(|e| {
                exports::wrpc::keyvalue::atomics::Error::Other(format!("redis error {:?}", e))
            }))
    }
}

impl Provider for SecretsExampleProvider {}
