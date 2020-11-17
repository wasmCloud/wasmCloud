use crate::generated::core::{deserialize, serialize};
use crate::messagebus::OP_BIND_ACTOR;
use crate::messagebus::{AdvertiseBinding, LatticeProvider};
use crate::{Invocation, InvocationResponse, WasccEntity, SYSTEM_ACTOR};
use actix::prelude::*;
use std::sync::Arc;
use wascap::prelude::KeyPair;
/*
pub(crate) fn do_rpc(l: &Box<dyn LatticeProvider>, inv: &Invocation) -> InvocationResponse {
    match l.rpc(&inv) {
        Ok(ir) => ir,
        Err(e) => {
            println!("INVOKE ERR: {}", e);
            InvocationResponse::error(&inv, &format!("RPC failure: {}", e))
        }
    }
}
*/

pub(crate) async fn do_async_rpc(
    l: &Box<dyn LatticeProvider>,
    inv: &Invocation,
) -> InvocationResponse {
    /*let nc = nats::asynk::connect("0.0.0.0:4222").await.unwrap();
    let hack = "wasmbus.rpc.distributedecho.MB4OLDIC3TCZ4Q4TGGOVAZC43VXFE2JQVRAXQMQFXUCREOOFEKOKZTY2";
    let res = nc.request(&hack,&serialize(inv).unwrap()).await;
    match res {
        Ok(r) => {
            let resp: InvocationResponse = deserialize(&r.data).unwrap();
            resp
        },
        Err(e) => InvocationResponse::error(&inv, &format!("RPC failure: {}", e))
    }*/
    match l.rpc(&inv).await {
        Ok(ir) => ir,
        Err(e) => {
            println!("INVOKE ERR: {}", e);
            InvocationResponse::error(&inv, &format!("RPC failure: {}", e))
        }
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
