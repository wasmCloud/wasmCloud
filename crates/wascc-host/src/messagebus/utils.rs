use actix::prelude::*;
use crate::messagebus::{LatticeProvider, AdvertiseBinding};
use crate::{Invocation, InvocationResponse, WasccEntity, SYSTEM_ACTOR};
use wascap::prelude::KeyPair;
use crate::messagebus::OP_BIND_ACTOR;

pub(crate) fn do_rpc(l: &Box<dyn LatticeProvider>, inv: &Invocation) -> InvocationResponse {
    match l.rpc(&inv) {
        Ok(ir) => ir,
        Err(e) => InvocationResponse::error(&inv, &format!("RPC failure: {}", e)),
    }
}

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