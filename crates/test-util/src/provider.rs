//! Provider management utilities for use during testing

use std::pin::pin;
use std::time::Duration;

use anyhow::{anyhow, ensure, Context as _, Result};
use serde::Deserialize;
use tokio::time::interval;
use tokio_stream::wrappers::IntervalStream;
use tokio_stream::StreamExt;
use tracing::warn;
use wasmcloud_core::health_subject;

/// Helper method for deserializing content, so that we can easily switch out implementations
pub fn deserialize<'de, T: Deserialize<'de>>(buf: &'de [u8]) -> Result<T> {
    serde_json::from_slice(buf).context("failed to deserialize")
}

/// Arguments to [`assert_start_provider`]
pub struct StartProviderArgs<'a> {
    /// [`wasmcloud_control_interface::Client`] to use when starting the provider
    pub client: &'a wasmcloud_control_interface::Client,
    /// ID of the host on which the provider should be started
    pub host_id: &'a str,
    /// ID of the provider that should be started
    pub provider_id: &'a str,
    /// Image ref of the provider to start
    pub provider_ref: &'a str,
    /// Named configuration to provide attach to the provider
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

/// Start a provider, ensuring that the provider starts properly
///
/// # Errors
///
/// Returns an `Err` if the provider fails to start
pub async fn assert_start_provider(
    StartProviderArgs {
        client,
        host_id,
        provider_id,
        provider_ref,
        config,
    }: StartProviderArgs<'_>,
) -> Result<()> {
    let lattice = client.lattice();
    let rpc_client = client.nats_client();
    let resp = client
        .start_provider(host_id, provider_ref, provider_id, None, config)
        .await
        .map_err(|e| anyhow!(e).context("failed to start provider"))?;
    ensure!(resp.succeeded());

    let res = pin!(IntervalStream::new(interval(Duration::from_secs(1)))
        .take(30)
        .then(|_| rpc_client.request(health_subject(lattice, provider_id), "".into(),))
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
