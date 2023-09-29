use std::collections::HashMap;

use anyhow::{bail, Context, Result};
use tokio::time::Duration;
use wasmcloud_control_interface::{Client as CtlClient, CtlOperationAck};

use crate::{
    common::boxed_err_to_anyhow,
    config::DEFAULT_START_ACTOR_TIMEOUT_MS,
    wait::{
        wait_for_actor_start_event, wait_for_actor_stop_event, ActorStoppedInfo, FindEventOutcome,
    },
};

/// Arguments required when starting an actor
pub struct StartActorArgs<'a> {
    pub ctl_client: &'a CtlClient,
    pub host_id: &'a str,
    pub actor_ref: &'a str,
    pub count: u16,
    pub skip_wait: bool,
    pub timeout_ms: Option<u64>,
}

/// Information related to an actor start
pub struct ActorStartedInfo {
    pub host_id: String,
    pub actor_ref: String,
    pub actor_id: Option<String>,
}

/// Start a Wasmcloud actor
pub async fn start_actor(
    StartActorArgs {
        ctl_client,
        host_id,
        actor_ref,
        count,
        skip_wait,
        timeout_ms,
    }: StartActorArgs<'_>,
) -> Result<ActorStartedInfo> {
    // If timeout isn't supplied, override with a longer timeout for starting actor
    let timeout_ms = timeout_ms.unwrap_or(DEFAULT_START_ACTOR_TIMEOUT_MS);

    // Create a receiver to use with the client
    let mut receiver = ctl_client
        .events_receiver()
        .await
        .map_err(boxed_err_to_anyhow)
        .context("Failed to get lattice event channel")?;

    // Start the actor
    let ack = ctl_client
        .start_actor(host_id, actor_ref, count, None)
        .await
        .map_err(boxed_err_to_anyhow)
        .with_context(|| format!("Failed to start actor: {}", actor_ref))?;

    if !ack.accepted {
        bail!("Start actor ack not accepted: {}", ack.error);
    }

    // If skip_wait is specified, return incomplete information immediately
    if skip_wait {
        return Ok(ActorStartedInfo {
            host_id: host_id.into(),
            actor_ref: actor_ref.into(),
            actor_id: None,
        });
    }

    // Wait for the actor to start
    let event = wait_for_actor_start_event(
        &mut receiver,
        Duration::from_millis(timeout_ms),
        host_id.into(),
        actor_ref.into(),
    )
    .await
    .with_context(|| {
        format!(
            "Timed out waitng for start event for actor [{}] on host [{}]",
            actor_ref, host_id
        )
    })?;

    match event {
        FindEventOutcome::Success(info) => Ok(info),
        FindEventOutcome::Failure(err) => Err(err).with_context(|| {
            format!(
                "Failed to start actor [{}] on host [{}]",
                actor_ref, host_id
            )
        }),
    }
}

/// Scale a Wasmcloud actor on a given host
pub async fn scale_actor(
    client: &CtlClient,
    host_id: &str,
    actor_ref: &str,
    actor_id: &str,
    count: u16,
    annotations: Option<HashMap<String, String>>,
) -> Result<()> {
    let ack = client
        .scale_actor(host_id, actor_ref, actor_id, count, annotations)
        .await
        .map_err(boxed_err_to_anyhow)?;

    if !ack.accepted {
        bail!("Operation failed: {}", ack.error);
    }

    Ok(())
}

/// Stop an actor
pub async fn stop_actor(
    client: &CtlClient,
    host_id: &str,
    actor_id: &str,
    count: u16,
    annotations: Option<HashMap<String, String>>,
    timeout_ms: u64,
    skip_wait: bool,
) -> Result<ActorStoppedInfo> {
    let mut receiver = client
        .events_receiver()
        .await
        .map_err(boxed_err_to_anyhow)?;

    let ack = client
        .stop_actor(host_id, actor_id, count, annotations)
        .await
        .map_err(boxed_err_to_anyhow)?;

    if !ack.accepted {
        bail!("Operation failed: {}", ack.error);
    }

    if skip_wait {
        return Ok(ActorStoppedInfo {
            actor_id: actor_id.into(),
            host_id: host_id.into(),
        });
    }

    let event = wait_for_actor_stop_event(
        &mut receiver,
        Duration::from_millis(timeout_ms),
        host_id.to_string(),
        actor_id.to_string(),
    )
    .await?;

    match event {
        FindEventOutcome::Success(info) => Ok(info),
        FindEventOutcome::Failure(err) => Err(err),
    }
}

pub async fn update_actor(
    client: &CtlClient,
    host_id: &str,
    actor_id: &str,
    actor_ref: &str,
) -> Result<CtlOperationAck> {
    client
        .update_actor(host_id, actor_id, actor_ref, None)
        .await
        .map_err(boxed_err_to_anyhow)
}
