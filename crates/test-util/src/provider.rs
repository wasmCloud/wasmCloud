//! Provider management utilities for use during testing

use std::pin::pin;
use std::time::Duration;

use anyhow::{anyhow, ensure, Context as _, Result};
use nkeys::KeyPair;
use serde::Deserialize;
use tokio::time::interval;
use tokio_stream::wrappers::IntervalStream;
use tokio_stream::StreamExt;
use tracing::warn;
use url::Url;
use wasmcloud_control_interface::CtlResponse;

/// Helper method for deserializing content, so that we can easily switch out implementations
pub fn deserialize<'de, T: Deserialize<'de>>(buf: &'de [u8]) -> Result<T> {
    serde_json::from_slice(buf).context("failed to deserialize")
}

/// Arguments to [`assert_start_provider`]
pub struct StartProviderArgs<'a> {
    pub client: &'a wasmcloud_control_interface::Client,
    pub lattice: &'a str,
    pub host_key: &'a KeyPair,
    pub provider_key: &'a KeyPair,
    pub provider_id: &'a str,
    pub url: &'a Url,
    pub config: Vec<String>,
}

/// Response expected from a successful healthcheck
#[derive(Deserialize)]
#[serde(deny_unknown_fields)]
struct ProviderHealthCheckResponse {
    #[serde(default)]
    healthy: bool,
    #[serde(default)]
    message: Option<String>,
}

/// Start an actor, ensuring that the provider starts properly
///
/// # Errors
///
/// Returns an `Err` if the actor fails to start
pub async fn assert_start_provider(
    StartProviderArgs {
        client,
        lattice,
        host_key,
        provider_key,
        provider_id,
        url,
        config,
    }: StartProviderArgs<'_>,
) -> Result<()> {
    let rpc_client = client.nats_client();
    let CtlResponse {
        success, message, ..
    } = client
        .start_provider(
            &host_key.public_key(),
            url.as_ref(),
            provider_id,
            None,
            config,
        )
        .await
        .map_err(|e| anyhow!(e).context("failed to start provider"))?;
    ensure!(message == "");
    ensure!(success);

    let res = pin!(IntervalStream::new(interval(Duration::from_secs(1)))
        .take(30)
        .then(|_| rpc_client.request(
            format!(
                "wasmbus.rpc.{}.{}.health",
                lattice,
                provider_key.public_key(),
            ),
            "".into(),
        ))
        .filter_map(|res| {
            match res {
                Err(error) => {
                    warn!(?error, "failed to connect to provider");
                    None
                }
                Ok(res) => Some(res),
            }
        }))
    .next()
    .await
    .context("failed to perform health check request")?;

    let ProviderHealthCheckResponse { healthy, message } = deserialize(&res.payload)
        .map_err(|e| anyhow!(e).context("failed to decode health check response"))?;
    ensure!(message == None);
    ensure!(healthy);
    Ok(())
}
