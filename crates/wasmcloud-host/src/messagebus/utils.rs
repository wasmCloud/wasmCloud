use crate::generated::core::{deserialize, serialize};
use crate::messagebus::AdvertiseBinding;
use crate::messagebus::OP_BIND_ACTOR;
use crate::{Invocation, InvocationResponse, WasccEntity, SYSTEM_ACTOR};
use actix::prelude::*;
use std::sync::Arc;
use wascap::prelude::KeyPair;

pub(crate) fn generate_binding_invocation(
    t: &Recipient<Invocation>,
    msg: &AdvertiseBinding,
    key: &KeyPair,
    target: WasccEntity,
) -> RecipientRequest<Invocation> {
    let config = crate::generated::core::CapabilityConfiguration {
        module: msg.actor.to_string(),
        values: msg.values.clone(),
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
