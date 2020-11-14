use crate::messagebus::OP_BIND_ACTOR;
use crate::messagebus::{AdvertiseLink, LatticeProvider};
use crate::{Invocation, InvocationResponse, WasccEntity, SYSTEM_ACTOR};
use actix::prelude::*;
use wascap::prelude::KeyPair;

pub(crate) fn do_rpc(l: &Box<dyn LatticeProvider>, inv: &Invocation) -> InvocationResponse {
    match l.rpc(&inv) {
        Ok(ir) => ir,
        Err(e) => InvocationResponse::error(&inv, &format!("RPC failure: {}", e)),
    }
}

pub(crate) fn generate_link_invocation(
    t: &Recipient<Invocation>,
    msg: &AdvertiseLink,
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
