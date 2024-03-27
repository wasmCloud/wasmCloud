use core::num::NonZeroUsize;

use std::collections::{BTreeMap, HashMap};

use anyhow::Context;
use cloudevents::{EventBuilder, EventBuilderV10};
use serde_json::json;
use time::{format_description::well_known::Rfc3339, OffsetDateTime};
use tracing::instrument;
use ulid::Ulid;
use uuid::Uuid;
use wascap::jwt;

fn format_actor_claims(claims: &jwt::Claims<jwt::Actor>) -> serde_json::Value {
    let issuer = &claims.issuer;
    let not_before_human = "TODO";
    let expires_human = "TODO";
    if let Some(actor) = &claims.metadata {
        json!({
            "call_alias": actor.call_alias,
            "caps": actor.caps,
            "issuer": issuer,
            "tags": actor.tags,
            "name": actor.name,
            "version": actor.ver,
            "revision": actor.rev,
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

pub fn actor_scaled(
    claims: Option<&jwt::Claims<jwt::Actor>>,
    annotations: &BTreeMap<String, String>,
    host_id: impl AsRef<str>,
    max_instances: impl Into<usize>,
    image_ref: impl AsRef<str>,
    actor_id: impl AsRef<str>,
) -> serde_json::Value {
    if let Some(claims) = claims {
        json!({
            "public_key": claims.subject,
            "claims": format_actor_claims(claims),
            "annotations": annotations,
            "host_id": host_id.as_ref(),
            "image_ref": image_ref.as_ref(),
            "max_instances": max_instances.into(),
            "actor_id": actor_id.as_ref(),
        })
    } else {
        json!({
            "annotations": annotations,
            "host_id": host_id.as_ref(),
            "image_ref": image_ref.as_ref(),
            "max_instances": max_instances.into(),
            "actor_id": actor_id.as_ref(),
        })
    }
}

pub fn actor_scale_failed(
    claims: Option<&jwt::Claims<jwt::Actor>>,
    annotations: &BTreeMap<String, String>,
    host_id: impl AsRef<str>,
    image_ref: impl AsRef<str>,
    actor_id: impl AsRef<str>,
    max_instances: NonZeroUsize,
    error: &anyhow::Error,
) -> serde_json::Value {
    if let Some(claims) = claims {
        json!({
            "public_key": claims.subject,
            "actor_id": actor_id.as_ref(),
            "annotations": annotations,
            "host_id": host_id.as_ref(),
            "image_ref": image_ref.as_ref(),
            "max_instances": max_instances,
            "error": format!("{error:#}"),
        })
    } else {
        json!({
            "annotations": annotations,
            "actor_id": actor_id.as_ref(),
            "host_id": host_id.as_ref(),
            "image_ref": image_ref.as_ref(),
            "max_instances": max_instances,
            "error": format!("{error:#}"),
        })
    }
}

pub fn linkdef_set(
    link: &wasmcloud_control_interface::InterfaceLinkDefinition,
) -> serde_json::Value {
    json!({
        "source_id": link.source_id,
        "target": link.target,
        "name": link.name,
        "wit_namespace": link.wit_namespace,
        "wit_package": link.wit_package,
        "interfaces": link.interfaces,
        "source_config": link.source_config,
        "target_config": link.target_config,
    })
}

pub fn linkdef_deleted(
    source_id: impl AsRef<str>,
    name: impl AsRef<str>,
    wit_namespace: impl AsRef<str>,
    wit_package: impl AsRef<str>,
) -> serde_json::Value {
    json!({
        "source_id": source_id.as_ref(),
        "name": name.as_ref(),
        "wit_namespace": wit_namespace.as_ref(),
        "wit_package": wit_package.as_ref(),
    })
}

pub fn provider_started(
    claims: Option<&jwt::Claims<jwt::CapabilityProvider>>,
    annotations: &BTreeMap<String, String>,
    host_id: impl AsRef<str>,
    image_ref: impl AsRef<str>,
    provider_id: impl AsRef<str>,
) -> serde_json::Value {
    if let Some(claims) = claims {
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
                "not_before_human": "TODO",
                "expires_human": "TODO",
            },
            // TODO(#1548): remove these fields when we don't depend on them
            "instance_id": provider_id.as_ref(),
            "public_key": provider_id.as_ref(),
            "link_name": "default",
            "contract_id": metadata.map(|jwt::CapabilityProvider { capid, .. }| capid),
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

pub fn provider_start_failed(
    provider_ref: impl AsRef<str>,
    provider_id: impl AsRef<str>,
    error: &anyhow::Error,
) -> serde_json::Value {
    json!({
        "provider_ref": provider_ref.as_ref(),
        "provider_id": provider_id.as_ref(),
        "error": format!("{error:#}"),
        // TODO(#1548): remove this field when we don't depend on it
        "link_name": "default",
    })
}

pub fn provider_stopped(
    claims: Option<jwt::Claims<jwt::CapabilityProvider>>,
    annotations: &BTreeMap<String, String>,
    host_id: impl AsRef<str>,
    provider_id: impl AsRef<str>,
    reason: impl AsRef<str>,
) -> serde_json::Value {
    if let Some(claims) = claims {
        let metadata = claims.metadata.as_ref();
        json!({
            "host_id": host_id.as_ref(),
            "provider_id": provider_id.as_ref(),
            "annotations": annotations,
            "reason": reason.as_ref(),
            // TODO(#1548): remove these fields when we don't depend on them
            "instance_id": provider_id.as_ref(),
            "public_key": provider_id.as_ref(),
            "link_name": "default",
            "contract_id": metadata.map(|jwt::CapabilityProvider { capid, .. }| capid),
        })
    } else {
        json!({
            "host_id": host_id.as_ref(),
            "provider_id": provider_id.as_ref(),
            "annotations": annotations,
            "reason": reason.as_ref(),
        })
    }
}

pub fn provider_health_check(
    host_id: impl AsRef<str>,
    provider_id: impl AsRef<str>,
) -> serde_json::Value {
    json!({
        "host_id": host_id.as_ref(),
        "provider_id": provider_id.as_ref(),
    })
}

pub fn config_set(config_name: impl AsRef<str>) -> serde_json::Value {
    json!({
        "config_name": config_name.as_ref(),
    })
}

pub fn config_deleted(config_name: impl AsRef<str>) -> serde_json::Value {
    json!({
        "config_name": config_name.as_ref(),
    })
}

pub fn labels_changed(
    host_id: impl AsRef<str>,
    labels: impl Into<HashMap<String, String>>,
) -> serde_json::Value {
    json!({
        "host_id": host_id.as_ref(),
        "labels": labels.into(),
    })
}

#[instrument(level = "debug", skip(event_builder, ctl_nats, data))]
pub(crate) async fn publish(
    event_builder: &EventBuilderV10,
    ctl_nats: &async_nats::Client,
    lattice: &str,
    name: &str,
    data: serde_json::Value,
) -> anyhow::Result<()> {
    let now = OffsetDateTime::now_utc()
        .format(&Rfc3339)
        .context("failed to format current time")?;
    let ev = event_builder
        .clone()
        .ty(format!("com.wasmcloud.lattice.{name}"))
        .id(Uuid::from_u128(Ulid::new().into()).to_string())
        .time(now)
        .data("application/json", data)
        .build()
        .context("failed to build cloud event")?;
    let ev = serde_json::to_vec(&ev).context("failed to serialize event")?;
    ctl_nats
        .publish(format!("wasmbus.evt.{lattice}.{name}"), ev.into())
        .await
        .with_context(|| format!("failed to publish `{name}` event"))
}
