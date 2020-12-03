use crate::dispatch::{
    CONFIG_WASCC_CLAIMS_CAPABILITIES, CONFIG_WASCC_CLAIMS_EXPIRES, CONFIG_WASCC_CLAIMS_ISSUER,
    CONFIG_WASCC_CLAIMS_NAME, CONFIG_WASCC_CLAIMS_TAGS,
};
use crate::messagebus::AdvertiseLink;
use crate::messagebus::OP_BIND_ACTOR;
use crate::{Invocation, WasccEntity, SYSTEM_ACTOR};
use actix::prelude::*;
use std::collections::HashMap;
use wascap::jwt::Claims;
use wascap::prelude::KeyPair;

// TODO: add wascc internal claims to the config values
pub(crate) fn generate_link_invocation(
    t: &Recipient<Invocation>,
    //msg: &Advertiselink,
    actor: &str,
    values: HashMap<String, String>,
    key: &KeyPair,
    target: WasccEntity,
    claims: Claims<wascap::jwt::Actor>,
) -> RecipientRequest<Invocation> {
    // Add "hidden" configuration values to the config hashmap that
    // contain the issuer, capabilities list, name, and tags from
    // the source actor
    let mut values = values.clone();
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
        values: values.clone(),
    };
    let inv = Invocation::new(
        key,
        WasccEntity::Actor(SYSTEM_ACTOR.to_string()),
        target,
        OP_BIND_ACTOR,
        crate::generated::core::serialize(&config).unwrap(),
    );

    t.send(inv)
}
