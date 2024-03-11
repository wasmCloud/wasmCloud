//! Actor management utilities for use during testing

use std::{
    collections::HashMap,
    num::{NonZeroU32, NonZeroUsize},
    path::Path,
};

use anyhow::{anyhow, bail, ensure, Context as _, Result};
use nkeys::KeyPair;
use serde::Deserialize;
use std::time::Duration;
use tokio::fs;

use wascap::{jwt, wasm::extract_claims};
use wasmcloud_control_interface::{Client as WasmCloudCtlClient, CtlResponse};

/// This is a *partial* struct for the ActorScaled event, which normally consists of more fields
#[derive(Deserialize)]
struct ActorScaledEvent {
    pub max_instances: NonZeroUsize,
}

/// Given a path to an actor on disks, extract claims
pub async fn extract_actor_claims(
    wasm_binary_path: impl AsRef<Path>,
) -> Result<jwt::Claims<jwt::Actor>> {
    let wasm_binary_path = wasm_binary_path.as_ref();
    let jwt::Token { claims, .. } = extract_claims(fs::read(wasm_binary_path).await?)
        .context("failed to extract kv http smithy actor claims")?
        .context("component actor claims missing")?;
    Ok(claims)
}

/// Start an actor, ensuring that the actor starts properly
pub async fn assert_start_actor(
    ctl_client: impl Into<&WasmCloudCtlClient>,
    host_key: impl AsRef<KeyPair>,
    url: impl AsRef<str>,
    actor_id: impl AsRef<str>,
    count: u32,
    config: Vec<String>,
) -> Result<()> {
    let ctl_client = ctl_client.into();

    let mut receiver = ctl_client
        .events_receiver(vec!["actor_scaled".into()])
        .await
        .map_err(|e| anyhow!(e))?;

    let host_key = host_key.as_ref();

    let CtlResponse {
        success, message, ..
    } = ctl_client
        .scale_actor(
            &host_key.public_key(),
            url.as_ref(),
            actor_id.as_ref(),
            count,
            None,
            config,
        )
        .await
        .map_err(|e| anyhow!(e).context("failed to start actor"))?;
    ensure!(message == "");
    ensure!(success);

    tokio::select! {
        _ = receiver.recv() => {},
        _ = tokio::time::sleep(Duration::from_secs(10)) => {
            bail!("timed out waiting for actor started event");
        },
    }

    Ok(())
}

/// Scale an actor, ensuring that the scale up/down was successful
#[allow(clippy::too_many_arguments)]
pub async fn assert_scale_actor(
    ctl_client: impl Into<&WasmCloudCtlClient>,
    host_key: impl AsRef<KeyPair>,
    url: impl AsRef<str>,
    actor_id: impl AsRef<str>,
    annotations: Option<HashMap<String, String>>,
    count: u32,
    config: Vec<String>,
) -> anyhow::Result<()> {
    let host_key = host_key.as_ref();
    let ctl_client = ctl_client.into();

    let mut receiver = ctl_client
        .events_receiver(vec!["actor_scaled".into()])
        .await
        .map_err(|e| anyhow!(e))?;

    let expected_count =
        NonZeroUsize::try_from(NonZeroU32::new(count).context("failed to create nonzero u32")?)
            .context("failed to convert nonzero u32 to nonzero usize")?;
    let CtlResponse {
        success, message, ..
    } = ctl_client
        .scale_actor(
            &host_key.public_key(),
            url.as_ref(),
            actor_id.as_ref(),
            count,
            annotations,
            config,
        )
        .await
        .map_err(|e| anyhow!(e).context("failed to start actor"))?;
    ensure!(message == "");
    ensure!(success);

    tokio::select! {
        event = receiver.recv() => {
                let (_,_, Some(event_data)) = event.context("failed to get event")?.take_data() else {
                    bail!("failed to take data");
                };
                let ase: ActorScaledEvent = serde_json::from_value(TryInto::<serde_json::Value>::try_into(event_data).context("failed to parse event into JSON value")?).context("failed to convert value to")?;
                assert_eq!(ase.max_instances, expected_count);
        }
        _ = tokio::time::sleep(Duration::from_secs(10)) => {
            bail!("timed out waiting for actor scale event");
        },
    }

    Ok(())
}
