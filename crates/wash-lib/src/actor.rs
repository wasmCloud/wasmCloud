use std::collections::HashMap;

use anyhow::{bail, Context, Result};
use tokio::time::Duration;
use wasmcloud_control_interface::{Client as CtlClient, CtlResponse};

use crate::{
    common::boxed_err_to_anyhow,
    config::DEFAULT_START_ACTOR_TIMEOUT_MS,
    wait::{wait_for_actor_scaled_event, FindEventOutcome},
};

/// Information related to an actor scale
pub struct ActorScaledInfo {
    pub host_id: String,
    pub actor_ref: String,
    pub actor_id: String,
}

/// Arguments required when scaling an actor
///
/// # Properties
/// * `client` - The control interface client
/// * `host_id` - The ID of the host where the actor is running
/// * `actor_id` - The ID of the actor to scale
/// * `actor_ref` - The reference of the actor to scale
/// * `max_instances` - The maximum number of instances to scale to
/// * `annotations` - Optional annotations to apply to the actor
pub struct ScaleActorArgs<'a> {
    /// The control interface client
    pub client: &'a CtlClient,
    /// The ID of the host where the actor is running
    pub host_id: &'a str,
    /// The ID of the actor to scale
    pub actor_id: &'a str,
    /// The reference of the actor to scale
    pub actor_ref: &'a str,
    /// The maximum number of instances to scale to
    pub max_instances: u32,
    /// Optional annotations to apply to the actor
    pub annotations: Option<HashMap<String, String>>,
    /// List of named configuration to apply to the actor, may be empty
    pub config: Vec<String>,
    /// Whether to wait for the actor to scale
    pub skip_wait: bool,
    /// The timeout for waiting for the actor to scale
    pub timeout_ms: Option<u64>,
}

/// Scale a Wasmcloud actor on a given host
pub async fn scale_actor(
    ScaleActorArgs {
        client,
        host_id,
        actor_id,
        actor_ref,
        max_instances,
        annotations,
        config,
        skip_wait,
        timeout_ms,
    }: ScaleActorArgs<'_>,
) -> Result<ActorScaledInfo> {
    // If timeout isn't supplied, override with a longer timeout for starting actor
    let timeout_ms = timeout_ms.unwrap_or(DEFAULT_START_ACTOR_TIMEOUT_MS);

    // Create a receiver to use with the client
    let mut receiver = client
        .events_receiver(vec![
            "actor_scaled".to_string(),
            "actor_scale_failed".to_string(),
        ])
        .await
        .map_err(boxed_err_to_anyhow)
        .context("Failed to get lattice event channel")?;

    let ack = client
        .scale_actor(
            host_id,
            actor_ref,
            actor_id,
            max_instances,
            annotations,
            config,
        )
        .await
        .map_err(boxed_err_to_anyhow)?;

    if !ack.success {
        bail!("Operation failed: {}", ack.message);
    }

    // If skip_wait is specified, return incomplete information immediately
    if skip_wait {
        return Ok(ActorScaledInfo {
            host_id: host_id.into(),
            actor_ref: actor_ref.into(),
            actor_id: actor_id.into(),
        });
    }

    // Wait for the actor to start
    let event = wait_for_actor_scaled_event(
        &mut receiver,
        Duration::from_millis(timeout_ms),
        host_id.into(),
        actor_ref.into(),
    )
    .await
    .with_context(|| {
        format!(
            "Timed out waiting for start event for actor [{}] on host [{}]",
            actor_ref, host_id
        )
    })?;

    match event {
        FindEventOutcome::Success(info) => Ok(info),
        FindEventOutcome::Failure(err) => Err(err)
            .with_context(|| format!("Failed to scale actor [{actor_id}] on host [{host_id}]",)),
    }
}

pub async fn update_actor(
    client: &CtlClient,
    host_id: &str,
    actor_id: &str,
    actor_ref: &str,
) -> Result<CtlResponse<()>> {
    client
        .update_actor(host_id, actor_id, actor_ref, None)
        .await
        .map_err(boxed_err_to_anyhow)
}
