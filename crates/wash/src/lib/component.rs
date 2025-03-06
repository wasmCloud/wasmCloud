use std::collections::{BTreeMap, HashMap};

use anyhow::{bail, Context, Result};
use tokio::time::Duration;
use wasmcloud_control_interface::{Client as CtlClient, CtlResponse};

use crate::lib::common::boxed_err_to_anyhow;
use crate::lib::config::DEFAULT_START_COMPONENT_TIMEOUT_MS;
use crate::lib::wait::{wait_for_component_scaled_event, FindEventOutcome};

/// Information related to a component scale
pub struct ComponentScaledInfo {
    pub host_id: String,
    pub component_ref: String,
    pub component_id: String,
}

/// Arguments required when scaling an component
///
/// # Properties
/// * `client` - The control interface client
/// * `host_id` - The ID of the host where the component is running
/// * `component_id` - The ID of the component to scale
/// * `component_ref` - The reference of the component to scale
/// * `max_instances` - The maximum number of instances to scale to
/// * `annotations` - Optional annotations to apply to the component
pub struct ScaleComponentArgs<'a> {
    /// The control interface client
    pub client: &'a CtlClient,
    /// The ID of the host where the component is running
    pub host_id: &'a str,
    /// The ID of the component to scale
    pub component_id: &'a str,
    /// The reference of the component to scale
    pub component_ref: &'a str,
    /// The maximum number of instances to scale to
    pub max_instances: u32,
    /// Optional annotations to apply to the component
    pub annotations: Option<HashMap<String, String>>,
    /// List of named configuration to apply to the component, may be empty
    pub config: Vec<String>,
    /// Whether to wait for the component to scale
    pub skip_wait: bool,
    /// The timeout for waiting for the component to scale
    pub timeout_ms: Option<u64>,
}

/// Scale a Wasmcloud component on a given host
pub async fn scale_component(
    ScaleComponentArgs {
        client,
        host_id,
        component_id,
        component_ref,
        max_instances,
        annotations,
        config,
        skip_wait,
        timeout_ms,
    }: ScaleComponentArgs<'_>,
) -> Result<ComponentScaledInfo> {
    // If timeout isn't supplied, override with a longer timeout for starting component
    let timeout_ms = timeout_ms.unwrap_or(DEFAULT_START_COMPONENT_TIMEOUT_MS);

    // Create a receiver to use with the client
    let mut receiver = client
        .events_receiver(vec![
            "component_scaled".to_string(),
            "component_scale_failed".to_string(),
        ])
        .await
        .map_err(boxed_err_to_anyhow)
        .context("Failed to get lattice event channel")?;

    let ack = client
        .scale_component(
            host_id,
            component_ref,
            component_id,
            max_instances,
            annotations.map(BTreeMap::from_iter),
            config,
        )
        .await
        .map_err(boxed_err_to_anyhow)?;

    if !ack.succeeded() {
        bail!("Operation failed: {}", ack.message());
    }

    // If skip_wait is specified, return incomplete information immediately
    if skip_wait {
        return Ok(ComponentScaledInfo {
            host_id: host_id.into(),
            component_ref: component_ref.into(),
            component_id: component_id.into(),
        });
    }

    // Wait for the component to start
    let event = wait_for_component_scaled_event(
        &mut receiver,
        Duration::from_millis(timeout_ms),
        host_id,
        component_ref,
    )
    .await
    .with_context(|| {
        format!(
            "Timed out waiting for start event for component [{component_ref}] on host [{host_id}]"
        )
    })?;

    match event {
        FindEventOutcome::Success(info) => Ok(info),
        FindEventOutcome::Failure(err) => Err(err).with_context(|| {
            format!("Failed to scale component [{component_id}] on host [{host_id}]",)
        }),
    }
}

pub async fn update_component(
    client: &CtlClient,
    host_id: &str,
    component_id: &str,
    component_ref: &str,
) -> Result<CtlResponse<()>> {
    client
        .update_component(host_id, component_id, component_ref, None)
        .await
        .map_err(boxed_err_to_anyhow)
}
