use crate::dispatch::{
    CONFIG_WASCC_CLAIMS_CAPABILITIES, CONFIG_WASCC_CLAIMS_EXPIRES, CONFIG_WASCC_CLAIMS_ISSUER,
    CONFIG_WASCC_CLAIMS_NAME, CONFIG_WASCC_CLAIMS_TAGS,
};

use crate::messagebus::OP_BIND_ACTOR;
use crate::{Invocation, WasmCloudEntity, SYSTEM_ACTOR};
use actix::prelude::*;
use std::collections::HashMap;
use wascap::jwt::Claims;
use wascap::prelude::KeyPair;

pub(crate) fn generate_link_invocation_and_call(
    t: &Recipient<Invocation>,
    actor: &str,
    values: HashMap<String, String>,
    key: &KeyPair,
    target: WasmCloudEntity,
    claims: Claims<wascap::jwt::Actor>,
) -> RecipientRequest<Invocation> {
    // Add "hidden" configuration values to the config hashmap that
    // contain the issuer, capabilities list, name, and tags from
    // the source actor
    let mut values = values;
    values.insert(
        CONFIG_WASCC_CLAIMS_ISSUER.to_string(),
        claims.issuer.to_string(),
    );
    values.insert(
        CONFIG_WASCC_CLAIMS_CAPABILITIES.to_string(),
        claims
            .metadata
            .as_ref()
            .unwrap()
            .caps
            .as_ref()
            .unwrap_or(&Vec::new())
            .join(","),
    );
    values.insert(CONFIG_WASCC_CLAIMS_NAME.to_string(), claims.name());
    values.insert(
        CONFIG_WASCC_CLAIMS_EXPIRES.to_string(),
        claims.expires.unwrap_or(0).to_string(),
    );
    values.insert(
        CONFIG_WASCC_CLAIMS_TAGS.to_string(),
        claims
            .metadata
            .as_ref()
            .unwrap()
            .tags
            .as_ref()
            .unwrap_or(&Vec::new())
            .join(","),
    );

    let config = crate::generated::core::CapabilityConfiguration {
        module: actor.to_string(),
        values,
    };
    let inv = Invocation::new(
        key,
        WasmCloudEntity::Actor(SYSTEM_ACTOR.to_string()),
        target,
        OP_BIND_ACTOR,
        crate::generated::core::serialize(&config).unwrap(),
    );

    t.send(inv)
}

pub(crate) fn system_actor_claims() -> Claims<wascap::jwt::Actor> {
    Claims::<wascap::jwt::Actor>::new(
        SYSTEM_ACTOR.to_string(),
        "ACOJJN6WUP4ODD75XEBKKTCCUJJCY5ZKQ56XVKYK4BEJWGVAOOQHZMCW".to_string(),
        SYSTEM_ACTOR.to_string(),
        None,
        None,
        false,
        None,
        None,
        None,
    )
}
