use std::collections::{BTreeMap, HashMap};

use serde_json::json;
use wascap::jwt;
use wasmcloud_control_interface::Link;

/// A trait for publishing wasmbus events. This can be implemented by any transport or bus
/// implementation that can send the serialized event to the appropriate destination.
///
/// TODO(brooksmtownsend): file an issue for this: This trait can certainly be enhanced by adding methods specific to the event
#[async_trait::async_trait]
pub trait EventPublisher: Send + Sync {
    /// Publish an event that occurred in the host. The event name is the type of event being published, and the
    /// data is the payload of the event. It's up to the implementation to
    /// determine how to handle events. By default, this is a no-op.
    async fn publish_event(
        &self,
        _event_name: &str,
        _data: serde_json::Value,
    ) -> anyhow::Result<()> {
        Ok(())
    }
}

/// A default implementation of the EventPublisher trait that does nothing.
/// This is useful for testing or when no event publishing is required.
#[derive(Default)]
pub struct DefaultEventPublisher {}
impl EventPublisher for DefaultEventPublisher {}

fn format_component_claims(claims: &jwt::Claims<jwt::Component>) -> serde_json::Value {
    let issuer = &claims.issuer;
    let not_before_human = claims
        .not_before
        .map(|n| n.to_string())
        .unwrap_or_else(|| "never".to_string());
    let expires_human = claims
        .expires
        .map(|n| n.to_string())
        .unwrap_or_else(|| "never".to_string());
    if let Some(component) = &claims.metadata {
        json!({
            "call_alias": component.call_alias,
            "issuer": issuer,
            "tags": component.tags,
            "name": component.name,
            "version": component.ver,
            "revision": component.rev,
            "not_before_human": not_before_human,
            "expires_human": expires_human,
        })
    } else {
        json!({
            "issuer": issuer,
            "not_before_human": not_before_human,
            "expires_human": expires_human,
        })
    }
}

/// Generates an event payload for when a component is scaled
///
/// # Arguments
/// * `claims` - Optional component claims
/// * `annotations` - Key-value pairs of metadata annotations
/// * `host_id` - ID of the host where scaling occurred
/// * `max_instances` - Maximum number of instances to scale to
/// * `image_ref` - Reference to the component image
/// * `component_id` - Unique identifier for the component
///
/// # Returns
/// JSON object containing scaling details and component metadata
pub fn component_scaled(
    claims: Option<&jwt::Claims<jwt::Component>>,
    annotations: &BTreeMap<String, String>,
    host_id: impl AsRef<str>,
    max_instances: impl Into<usize>,
    image_ref: impl AsRef<str>,
    component_id: impl AsRef<str>,
) -> serde_json::Value {
    if let Some(claims) = claims {
        json!({
            "public_key": claims.subject,
            "claims": format_component_claims(claims),
            "annotations": annotations,
            "host_id": host_id.as_ref(),
            "image_ref": image_ref.as_ref(),
            "max_instances": max_instances.into(),
            "component_id": component_id.as_ref(),
        })
    } else {
        json!({
            "annotations": annotations,
            "host_id": host_id.as_ref(),
            "image_ref": image_ref.as_ref(),
            "max_instances": max_instances.into(),
            "component_id": component_id.as_ref(),
        })
    }
}

/// Generates an event payload for when component scaling fails
///
/// # Arguments
/// * `claims` - Optional component claims
/// * `annotations` - Key-value pairs of metadata annotations
/// * `host_id` - ID of the host where scaling failed
/// * `image_ref` - Reference to the component image
/// * `component_id` - Unique identifier for the component
/// * `max_instances` - Target number of instances that failed to scale
/// * `error` - The error that caused the scaling failure
///
/// # Returns
/// JSON object containing scaling failure details and error information
pub fn component_scale_failed(
    claims: Option<&jwt::Claims<jwt::Component>>,
    annotations: &BTreeMap<String, String>,
    host_id: impl AsRef<str>,
    image_ref: impl AsRef<str>,
    component_id: impl AsRef<str>,
    max_instances: u32,
    error: &anyhow::Error,
) -> serde_json::Value {
    if let Some(claims) = claims {
        json!({
            "public_key": claims.subject,
            "component_id": component_id.as_ref(),
            "annotations": annotations,
            "host_id": host_id.as_ref(),
            "image_ref": image_ref.as_ref(),
            "max_instances": max_instances,
            "error": format!("{error:#}"),
        })
    } else {
        json!({
            "annotations": annotations,
            "component_id": component_id.as_ref(),
            "host_id": host_id.as_ref(),
            "image_ref": image_ref.as_ref(),
            "max_instances": max_instances,
            "error": format!("{error:#}"),
        })
    }
}

/// Generates an event payload for when a link definition is set
///
/// # Arguments
/// * `link` - Link definition containing source, target, and interface information
///
/// # Returns
/// JSON object containing complete link definition details
pub fn linkdef_set(link: &Link) -> serde_json::Value {
    json!({
        "source_id": link.source_id(),
        "target": link.target(),
        "name": link.name(),
        "wit_namespace": link.wit_namespace(),
        "wit_package": link.wit_package(),
        "interfaces": link.interfaces(),
        "source_config": link.source_config(),
        "target_config": link.target_config(),
    })
}

/// Generates an event payload for when setting a link definition fails
///
/// # Arguments
/// * `link` - Link definition that failed to be set
/// * `error` - The error that caused the link definition failure
///
/// # Returns
/// JSON object containing link definition details and error information
pub fn linkdef_set_failed(link: &Link, error: &anyhow::Error) -> serde_json::Value {
    json!({
        "source_id": link.source_id(),
        "target": link.target(),
        "name": link.name(),
        "wit_namespace": link.wit_namespace(),
        "wit_package": link.wit_package(),
        "interfaces": link.interfaces(),
        "source_config": link.source_config(),
        "target_config": link.target_config(),
        "error": format!("{error:#}"),
    })
}

/// Generates an event payload for when a link definition is deleted
///
/// # Arguments
/// * `source_id` - ID of the source component
/// * `target` - Optional target component ID
/// * `name` - Name of the link
/// * `wit_namespace` - WIT namespace for the link
/// * `wit_package` - WIT package name
/// * `interfaces` - Optional list of interface names
///
/// # Returns
/// JSON object containing link deletion details
pub fn linkdef_deleted(
    source_id: impl AsRef<str>,
    target: Option<&String>,
    name: impl AsRef<str>,
    wit_namespace: impl AsRef<str>,
    wit_package: impl AsRef<str>,
    interfaces: Option<&Vec<String>>,
) -> serde_json::Value {
    // Target and interfaces aren't known if the link didn't exist, so we omit them from the
    // event data in that case.
    if let (Some(target), Some(interfaces)) = (target, interfaces) {
        json!({
            "source_id": source_id.as_ref(),
            "target": target,
            "name": name.as_ref(),
            "wit_namespace": wit_namespace.as_ref(),
            "wit_package": wit_package.as_ref(),
            "interfaces": interfaces,
        })
    } else {
        json!({
            "source_id": source_id.as_ref(),
            "name": name.as_ref(),
            "wit_namespace": wit_namespace.as_ref(),
            "wit_package": wit_package.as_ref(),
        })
    }
}

/// Generates an event payload for when a provider starts
///
/// # Arguments
/// * `claims` - Optional capability provider claims
/// * `annotations` - Key-value pairs of metadata annotations
/// * `host_id` - ID of the host where provider started
/// * `image_ref` - Reference to the provider image
/// * `provider_id` - Unique identifier for the provider
///
/// # Returns
/// JSON object containing provider startup details and metadata
pub fn provider_started(
    claims: Option<&jwt::Claims<jwt::CapabilityProvider>>,
    annotations: &BTreeMap<String, String>,
    host_id: impl AsRef<str>,
    image_ref: impl AsRef<str>,
    provider_id: impl AsRef<str>,
) -> serde_json::Value {
    if let Some(claims) = claims {
        let not_before_human = claims
            .not_before
            .map(|n| n.to_string())
            .unwrap_or_else(|| "never".to_string());
        let expires_human = claims
            .expires
            .map(|n| n.to_string())
            .unwrap_or_else(|| "never".to_string());
        let metadata = claims.metadata.as_ref();
        json!({
            "host_id": host_id.as_ref(),
            "image_ref": image_ref.as_ref(),
            "provider_id": provider_id.as_ref(),
            "annotations": annotations,
            "claims": {
                "issuer": &claims.issuer,
                "tags": None::<Vec<()>>, // present in OTP, but hardcoded to `None`
                "name": metadata.map(|jwt::CapabilityProvider { name, .. }| name),
                "version": metadata.map(|jwt::CapabilityProvider { ver, .. }| ver),
                "not_before_human": not_before_human,
                "expires_human": expires_human,
            },
            // TODO(#1548): remove these fields when we don't depend on them
            "instance_id": provider_id.as_ref(),
            "public_key": provider_id.as_ref(),
            "link_name": "default",
        })
    } else {
        json!({
            "host_id": host_id.as_ref(),
            "image_ref": image_ref.as_ref(),
            "provider_id": provider_id.as_ref(),
            "annotations": annotations,
        })
    }
}

/// Generates an event payload for when a provider fails to start
///
/// # Arguments
/// * `provider_ref` - Reference to the provider image
/// * `provider_id` - Unique identifier for the provider
/// * `host_id` - ID of the host where start failed
/// * `error` - The error that caused the start failure
///
/// # Returns
/// JSON object containing provider start failure details
pub fn provider_start_failed(
    provider_ref: impl AsRef<str>,
    provider_id: impl AsRef<str>,
    host_id: impl AsRef<str>,
    error: &anyhow::Error,
) -> serde_json::Value {
    json!({
        "provider_ref": provider_ref.as_ref(),
        "provider_id": provider_id.as_ref(),
        "host_id": host_id.as_ref(),
        "error": format!("{error:#}"),
        // TODO(#1548): remove this field when we don't depend on it
        "link_name": "default",
    })
}

/// Generates an event payload for when a provider stops
///
/// # Arguments
/// * `annotations` - Key-value pairs of metadata annotations
/// * `host_id` - ID of the host where provider stopped
/// * `provider_id` - Unique identifier for the provider
/// * `reason` - Reason for stopping the provider
///
/// # Returns
/// JSON object containing provider stop details
pub fn provider_stopped(
    annotations: &BTreeMap<String, String>,
    host_id: impl AsRef<str>,
    provider_id: impl AsRef<str>,
    reason: impl AsRef<str>,
) -> serde_json::Value {
    json!({
        "host_id": host_id.as_ref(),
        "provider_id": provider_id.as_ref(),
        "annotations": annotations,
        "reason": reason.as_ref(),
        // TODO(#1548): remove these fields when we don't depend on them
        "instance_id": provider_id.as_ref(),
        "public_key": provider_id.as_ref(),
        "link_name": "default",
    })
}

/// Generates an event payload for provider health checks
///
/// # Arguments
/// * `host_id` - ID of the host performing the health check
/// * `provider_id` - Unique identifier for the provider being checked
///
/// # Returns
/// JSON object containing health check details
pub fn provider_health_check(
    host_id: impl AsRef<str>,
    provider_id: impl AsRef<str>,
) -> serde_json::Value {
    json!({
        "host_id": host_id.as_ref(),
        "provider_id": provider_id.as_ref(),
    })
}

/// Generates an event payload for when a config is set
///
/// # Arguments
/// * `config_name` - Name of the configuration being set
///
/// # Returns
/// JSON object containing config set details
pub fn config_set(config_name: impl AsRef<str>) -> serde_json::Value {
    json!({
        "config_name": config_name.as_ref(),
    })
}

/// Generates an event payload for when a config is deleted
///
/// # Arguments
/// * `config_name` - Name of the configuration being deleted
///
/// # Returns
/// JSON object containing config deletion details
pub fn config_deleted(config_name: impl AsRef<str>) -> serde_json::Value {
    json!({
        "config_name": config_name.as_ref(),
    })
}

/// Generates an event payload for when host labels are changed
///
/// # Arguments
/// * `host_id` - ID of the host whose labels changed
/// * `labels` - New set of labels as key-value pairs
///
/// # Returns
/// JSON object containing updated label information
pub fn labels_changed(
    host_id: impl AsRef<str>,
    labels: impl Into<HashMap<String, String>>,
) -> serde_json::Value {
    json!({
        "host_id": host_id.as_ref(),
        "labels": labels.into(),
    })
}
