use std::collections::HashMap;
use std::sync::Arc;

use anyhow::{bail, Context as _};
use redis::aio::ConnectionManager;
use redis::AsyncCommands;
use tokio::sync::RwLock;
use tracing::info;
use tracing::warn;
use wasmcloud_provider_sdk::core::secrets::SecretValue;
use wasmcloud_provider_sdk::core::HostData;
use wasmcloud_provider_sdk::initialize_observability;
use wasmcloud_provider_sdk::serve_provider_exports;
use wasmcloud_provider_sdk::LinkConfig;
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
    #[allow(dead_code)]
    default_connection: Arc<RwLock<ConnectionManager>>,
    component_connections: Arc<RwLock<HashMap<String, ConnectionManager>>>,
}

impl SecretsExampleProvider {
    pub async fn run() -> anyhow::Result<()> {
        initialize_observability!("secrets-example-provider", None::<std::ffi::OsString>);

        let HostData {
            secrets, config, ..
        } = load_host_data().context("failed to load host data")?;
        let SecretValue::String(password) = secrets.get("default_redis_password").context(
            format!("default_redis_password secret not found: {:?}", secrets),
        )?
        else {
            bail!("password secret not a string")
        };
        let url = config
            .get("url")
            .cloned()
            .unwrap_or_else(|| DEFAULT_REDIS_URL.to_string());

        let conn_manager = connect_to_redis_authenticated(&password, &url)
            .await
            .context("failed to make authenticated connection to redis")?;
        let provider = SecretsExampleProvider {
            default_connection: Arc::new(RwLock::new(conn_manager)),
            component_connections: Arc::new(RwLock::new(HashMap::new())),
        };
        let shutdown = run_provider(provider.clone(), "secrets-example-provider")
            .await
            .context("failed to run provider")?;
        let connection = wasmcloud_provider_sdk::get_connection();
        let wrpc = connection
            .get_wrpc_client(connection.provider_key())
            .await?;
        serve_provider_exports(&wrpc, provider, shutdown, bindings::serve).await
    }
}

impl keyvalue::atomics::Handler<Option<Context>> for SecretsExampleProvider {
    async fn increment(
        &self,
        ctx: Option<Context>,
        _bucket: String,
        key: String,
        delta: u64,
    ) -> anyhow::Result<Result<u64, keyvalue::atomics::Error>> {
        let ctx = ctx.context("unexpectedly missing context")?;
        let connections = self.component_connections.read().await;
        let source_id = ctx
            .component
            .context("received invocation from unlinked component")?;
        let mut conn = connections
            .get(&source_id)
            .with_context(|| format!("failed to find redis connection for source [{source_id}]"))?
            .clone();

        Ok(conn
            .incr(key, delta)
            .await
            .map_err(|e| keyvalue::atomics::Error::Other(format!("redis error: {:?}", e))))
    }
}

impl Provider for SecretsExampleProvider {
    async fn receive_link_config_as_target(&self, config: LinkConfig<'_>) -> anyhow::Result<()> {
        info!(?config.source_id, "handling link for component");
        let SecretValue::String(password) = config
            .secrets
            .get("redis_password")
            .with_context(|| format!("password secret not found: {:?}", config.secrets))?
        else {
            bail!("password secret not a string")
        };

        let component_connection = connect_to_redis_authenticated(
            password,
            config.config.get("url").context("url secret not found")?,
        )
        .await
        .context("failed to make authenticated connection to redis")?;

        self.component_connections
            .write()
            .await
            .insert(config.source_id.to_string(), component_connection);

        Ok(())
    }
}

async fn connect_to_redis_authenticated(
    password: &str,
    url: &str,
) -> anyhow::Result<ConnectionManager> {
    match redis::Client::open(format!("redis://:{password}@{url}")) {
        Ok(client) => match client.get_connection_manager().await {
            Ok(conn) => {
                info!(url, "connected to redis with password");
                Ok(conn)
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
    }
}
