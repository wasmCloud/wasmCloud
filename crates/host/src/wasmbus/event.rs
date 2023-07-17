use core::num::NonZeroUsize;

use std::collections::BTreeMap;

use serde_json::json;
use uuid::Uuid;
use wascap::jwt;

fn format_claims(claims: &jwt::Claims<jwt::Actor>) -> serde_json::Value {
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
        "claims": format_claims(claims),
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
