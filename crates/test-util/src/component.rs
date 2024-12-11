//! Component management utilities for use during testing

use std::collections::BTreeMap;
use std::num::{NonZeroU32, NonZeroUsize};
use std::path::Path;

use anyhow::{anyhow, bail, ensure, Context as _, Result};
use nkeys::KeyPair;
use serde::Deserialize;
use std::time::Duration;
use tokio::fs;
use wasmcloud_control_interface::ComponentDescription;

use wascap::{jwt, wasm::extract_claims};

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
    ctl_client: impl Into<&wasmcloud_control_interface::Client>,
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
///
/// # Arguments
///
/// * `ctl_client` - The [`wasmcloud_control_interface::Client`] to use when scaling the component
/// * `host_id` - ID of the host
/// * `component_ref` - Image ref of the component that should be scaled
/// * `component_id` - ID of the component to be scaled
/// * `annotations` - Annotations to put on the component (if any)
/// * `count` - Number of components to scale to
/// * `config` - named configs to be associated with the component, if any
/// * `scale_timeout` - amount of time to allow for scale to complete
///
#[allow(clippy::too_many_arguments)]
pub async fn assert_scale_component(
    ctl_client: impl Into<&wasmcloud_control_interface::Client>,
    host_id: impl AsRef<str>,
    component_ref: impl AsRef<str>,
    component_id: impl AsRef<str>,
    annotations: Option<BTreeMap<String, String>>,
    count: u32,
    config: Vec<String>,
    scale_timeout: Duration,
) -> anyhow::Result<()> {
    let ctl_client = ctl_client.into();
    let host_id = host_id.as_ref();
    let component_ref = component_ref.as_ref();
    let component_id = component_id.as_ref();

    let mut receiver = ctl_client
        .events_receiver(vec!["component_scaled".into()])
        .await
        .map_err(|e| anyhow!(e))?;

    let expected_count =
        NonZeroUsize::try_from(NonZeroU32::new(count).context("failed to create nonzero u32")?)
            .context("failed to convert nonzero u32 to nonzero usize")?;
    let resp = ctl_client
        .scale_component(
            host_id,
            component_ref,
            component_id,
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
        () = tokio::time::sleep(scale_timeout) => {
            bail!("timed out waiting for component scale event");
        },
    }

    Ok(())
}

/// Wait for a component to be in a host's inventory (signaling start completion)
pub async fn wait_for_component_in_inventory(
    ctl_client: impl Into<&wasmcloud_control_interface::Client>,
    host_id: impl AsRef<str>,
    component_id: impl AsRef<str>,
    timeout: Duration,
) -> Result<ComponentDescription> {
    let ctl_client = ctl_client.into();
    let host_id = host_id.as_ref();
    let component_id = component_id.as_ref();

    tokio::time::timeout(timeout, async {
        loop {
            tokio::time::sleep(Duration::from_millis(500)).await;
            let Ok(resp) = ctl_client.get_host_inventory(host_id).await else {
                continue;
            };
            let Some(inv) = resp.data() else {
                continue;
            };
            // If the component is in the host inventory we can consider it started
            if let Some(c) = inv.components().iter().find(|c| c.id() == component_id) {
                return c.clone();
            }
        }
    })
    .await
    .context("failed to find component in given host")
}
