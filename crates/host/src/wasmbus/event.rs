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

pub fn actor_started(
    claims: &jwt::Claims<jwt::Actor>,
    annotations: &Option<BTreeMap<String, String>>,
    instance_id: Uuid,
    image_ref: impl AsRef<str>,
) -> serde_json::Value {
    json!({
        "public_key": claims.subject,
        "image_ref": image_ref.as_ref(),
        "api_version": "n/a",
        "instance_id": instance_id,
        "annotations": annotations,
        "claims": format_actor_claims(claims),
    })
}

pub fn actor_start_failed(actor_ref: impl AsRef<str>, error: &anyhow::Error) -> serde_json::Value {
    json!({
        "actor_ref": actor_ref.as_ref(),
        "error": format!("{error:#}"),
    })
}

pub fn actor_stopped(
    claims: &jwt::Claims<jwt::Actor>,
    annotations: &Option<BTreeMap<String, String>>,
    instance_id: Uuid,
) -> serde_json::Value {
    json!({
        "public_key": claims.subject,
        "instance_id": instance_id,
        "annotations": annotations,
    })
}

pub fn actors_started(
    claims: &jwt::Claims<jwt::Actor>,
    annotations: &Option<BTreeMap<String, String>>,
    host_id: impl AsRef<str>,
    count: impl Into<usize>,
    image_ref: impl AsRef<str>,
) -> serde_json::Value {
    json!({
        "public_key": claims.subject,
        "image_ref": image_ref.as_ref(),
        "annotations": annotations,
        "host_id": host_id.as_ref(),
        "claims": claims,
        "count": count.into(),
    })
}

pub fn actors_start_failed(
    claims: &jwt::Claims<jwt::Actor>,
    annotations: &Option<BTreeMap<String, String>>,
    host_id: impl AsRef<str>,
    image_ref: impl AsRef<str>,
    error: &anyhow::Error,
) -> serde_json::Value {
    json!({
        "public_key": claims.subject,
        "image_ref": image_ref.as_ref(),
        "annotations": annotations,
        "host_id": host_id.as_ref(),
        "error": format!("{error:#}"),
    })
}

pub fn actors_stopped(
    claims: &jwt::Claims<jwt::Actor>,
    annotations: &Option<BTreeMap<String, String>>,
    host_id: impl AsRef<str>,
    count: NonZeroUsize,
    remaining: usize,
) -> serde_json::Value {
    json!({
        "host_id": host_id.as_ref(),
        "public_key": claims.subject,
        "count": count,
        "remaining": remaining,
        "annotations": annotations,
    })
}

pub fn linkdef_set(
    id: impl AsRef<str>,
    actor_id: impl AsRef<str>,
    provider_id: impl AsRef<str>,
    link_name: impl AsRef<str>,
    contract_id: impl AsRef<str>,
    values: &HashMap<String, String>,
) -> serde_json::Value {
    json!({
        "id": id.as_ref(),
        "actor_id": actor_id.as_ref(),
        "provider_id": provider_id.as_ref(),
        "link_name": link_name.as_ref(),
        "contract_id": contract_id.as_ref(),
        "values": values,
    })
}

pub fn linkdef_deleted(
    id: impl AsRef<str>,
    actor_id: impl AsRef<str>,
    provider_id: impl AsRef<str>,
    link_name: impl AsRef<str>,
    contract_id: impl AsRef<str>,
    values: &HashMap<String, String>,
) -> serde_json::Value {
    json!({
        "id": id.as_ref(),
        "actor_id": actor_id.as_ref(),
        "provider_id": provider_id.as_ref(),
        "link_name": link_name.as_ref(),
        "contract_id": contract_id.as_ref(),
        "values": values,
    })
}

pub fn provider_started(
    claims: &jwt::Claims<jwt::CapabilityProvider>,
    annotations: &Option<BTreeMap<String, String>>,
    instance_id: Uuid,
    host_id: impl AsRef<str>,
    image_ref: impl AsRef<str>,
    link_name: impl AsRef<str>,
) -> serde_json::Value {
    let metadata = claims.metadata.as_ref();
    json!({
        "host_id": host_id.as_ref(),
        "public_key": claims.subject,
        "image_ref": image_ref.as_ref(),
        "link_name": link_name.as_ref(),
        "contract_id": metadata.map(|jwt::CapabilityProvider { capid, .. }| capid),
        "instance_id": instance_id,
        "annotations": annotations,
        "claims": {
            "issuer": &claims.issuer,
            "tags": None::<Vec<()>>, // present in OTP, but hardcoded to `None`
            "name": metadata.map(|jwt::CapabilityProvider { name, .. }| name),
            "version": metadata.map(|jwt::CapabilityProvider { ver, .. }| ver),
            "not_before_human": "TODO",
            "expires_human": "TODO",
        },
    })
}

pub fn provider_start_failed(
    provider_ref: impl AsRef<str>,
    link_name: impl AsRef<str>,
    error: &anyhow::Error,
) -> serde_json::Value {
    json!({
        "provider_ref": provider_ref.as_ref(),
        "link_name": link_name.as_ref(),
        "error": format!("{error:#}"),
    })
}

pub fn provider_stopped(
    claims: &jwt::Claims<jwt::CapabilityProvider>,
    annotations: &Option<BTreeMap<String, String>>,
    instance_id: Uuid,
    host_id: impl AsRef<str>,
    link_name: impl AsRef<str>,
    reason: impl AsRef<str>,
) -> serde_json::Value {
    let metadata = claims.metadata.as_ref();
    json!({
        "host_id": host_id.as_ref(),
        "public_key": claims.subject,
        "link_name": link_name.as_ref(),
        "contract_id": metadata.map(|jwt::CapabilityProvider { capid, .. }| capid),
        "instance_id": instance_id,
        "annotations": annotations,
        "reason": reason.as_ref(),
    })
}

pub fn provider_health_check(
    public_key: impl AsRef<str>,
    link_name: impl AsRef<str>,
    contract_id: impl AsRef<str>,
) -> serde_json::Value {
    json!({
        "public_key": public_key.as_ref(),
        "link_name": link_name.as_ref(),
        "contract_id": contract_id.as_ref(),
    })
}

#[instrument(skip(event_builder, ctl_nats, name))]
pub(crate) async fn publish(
    event_builder: &EventBuilderV10,
    ctl_nats: &async_nats::Client,
    lattice_prefix: &str,
    name: impl AsRef<str>,
    data: serde_json::Value,
) -> anyhow::Result<()> {
    let name = name.as_ref();
    let name = format!("com.wasmcloud.lattice.{name}");
    let now = OffsetDateTime::now_utc()
        .format(&Rfc3339)
        .context("failed to format current time")?;
    let ev = event_builder
        .clone()
        .ty(&name)
        .id(Uuid::from_u128(Ulid::new().into()).to_string())
        .time(now)
        .data("application/json", data)
        .build()
        .context("failed to build cloud event")?;
    let ev = serde_json::to_vec(&ev).context("failed to serialize event")?;
    ctl_nats
        .publish(format!("wasmbus.evt.{lattice_prefix}"), ev.into())
        .await
        .with_context(|| format!("failed to publish `{name}` event"))
}
