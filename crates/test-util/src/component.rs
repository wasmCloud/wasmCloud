//! Component management utilities for use during testing

use std::collections::BTreeMap;
use std::num::{NonZeroU32, NonZeroUsize};
use std::path::Path;

use anyhow::{anyhow, bail, ensure, Context as _, Result};
use nkeys::KeyPair;
use serde::Deserialize;
use std::time::Duration;
use tokio::fs;

use wascap::{jwt, wasm::extract_claims};
use wasmcloud_control_interface::Client as WasmCloudCtlClient;

/// This is a *partial* struct for the `ComponentScaled` event, which normally consists of more fields
#[derive(Deserialize)]
struct ComponentScaledEvent {
    pub max_instances: NonZeroUsize,
}

/// Given a path to an component on disks, extract claims
pub async fn extract_component_claims(
    wasm_binary_path: impl AsRef<Path>,
) -> Result<jwt::Claims<jwt::Component>> {
    let wasm_binary_path = wasm_binary_path.as_ref();
    let jwt::Token { claims, .. } = extract_claims(fs::read(wasm_binary_path).await?)
        .context("failed to extract kv http smithy component claims")?
        .context("component component claims missing")?;
    Ok(claims)
}

/// Start an component, ensuring that the component starts properly
pub async fn assert_start_component(
    ctl_client: impl Into<&WasmCloudCtlClient>,
    host_key: impl AsRef<KeyPair>,
    url: impl AsRef<str>,
    component_id: impl AsRef<str>,
    count: u32,
    config: Vec<String>,
) -> Result<()> {
    let ctl_client = ctl_client.into();

    let mut receiver = ctl_client
        .events_receiver(vec!["component_scaled".into()])
        .await
        .map_err(|e| anyhow!(e))?;

    let host_key = host_key.as_ref();

    let resp = ctl_client
        .scale_component(
            &host_key.public_key(),
            url.as_ref(),
            component_id.as_ref(),
            count,
            None,
            config,
        )
        .await
        .map_err(|e| anyhow!(e).context("failed to start component"))?;
    ensure!(resp.succeeded());

    tokio::select! {
        _ = receiver.recv() => {},
        () = tokio::time::sleep(Duration::from_secs(10)) => {
            bail!("timed out waiting for component started event");
        },
    }

    Ok(())
}

/// Scale an component, ensuring that the scale up/down was successful
#[allow(clippy::too_many_arguments)]
pub async fn assert_scale_component(
    ctl_client: impl Into<&WasmCloudCtlClient>,
    host_key: impl AsRef<KeyPair>,
    url: impl AsRef<str>,
    component_id: impl AsRef<str>,
    annotations: Option<BTreeMap<String, String>>,
    count: u32,
    config: Vec<String>,
) -> anyhow::Result<()> {
    let host_key = host_key.as_ref();
    let ctl_client = ctl_client.into();

    let mut receiver = ctl_client
        .events_receiver(vec!["component_scaled".into()])
        .await
        .map_err(|e| anyhow!(e))?;

    let expected_count =
        NonZeroUsize::try_from(NonZeroU32::new(count).context("failed to create nonzero u32")?)
            .context("failed to convert nonzero u32 to nonzero usize")?;
    let resp = ctl_client
        .scale_component(
            &host_key.public_key(),
            url.as_ref(),
            component_id.as_ref(),
            count,
            annotations,
            config,
        )
        .await
        .map_err(|e| anyhow!(e).context("failed to start component"))?;
    ensure!(resp.succeeded());

    tokio::select! {
        event = receiver.recv() => {
                let (_,_, Some(event_data)) = event.context("failed to get event")?.take_data() else {
                    bail!("failed to take data");
                };
                let ase: ComponentScaledEvent = serde_json::from_value(TryInto::<serde_json::Value>::try_into(event_data).context("failed to parse event into JSON value")?).context("failed to convert value to")?;
                assert_eq!(ase.max_instances, expected_count);
        }
        () = tokio::time::sleep(Duration::from_secs(10)) => {
            bail!("timed out waiting for component scale event");
        },
    }

    Ok(())
}
