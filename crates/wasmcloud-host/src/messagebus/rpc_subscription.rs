use crate::generated::core::{deserialize, serialize};
use crate::{Invocation, InvocationResponse, WasccEntity};
use actix::prelude::*;
use futures::StreamExt;
use std::sync::Arc;

#[derive(Message)]
#[rtype(result = "()")]
pub(crate) struct CreateSubscription {
    pub entity: WasccEntity,
    pub target: Recipient<Invocation>,
    pub nc: Arc<nats::asynk::Connection>,
    pub namespace: Option<String>,
}

#[derive(Message)]
#[rtype(result = "()")]
struct RpcInvocation {
    invocation: Option<Invocation>,
    reply: Option<String>,
}

#[derive(Default)]
pub(crate) struct RpcSubscription {
    target: Option<Recipient<Invocation>>,
    nc: Option<Arc<nats::asynk::Connection>>,
    ns_prefix: Option<String>,
}

impl Actor for RpcSubscription {
    type Context = Context<Self>;
}

impl Handler<CreateSubscription> for RpcSubscription {
    type Result = ResponseActFuture<Self, ()>;

    fn handle(&mut self, msg: CreateSubscription, _ctx: &mut Self::Context) -> Self::Result {
        info!("Creating lattice subscription for {}", msg.entity.url());
        self.target = Some(msg.target);
        self.nc = Some(msg.nc.clone());
        self.ns_prefix = msg.namespace;
        let nc = msg.nc.clone();
        let s = invoke_subject(&self.ns_prefix, &msg.entity);

        Box::pin(
            async move { nc.queue_subscribe(&s, &s).await }
                .into_actor(self)
                .map(|sub, _act, ctx| {
                    if let Ok(sub) = sub {
                        ctx.add_message_stream(sub.map(|m| {
                            let i = deserialize::<Invocation>(&m.data);
                            match i {
                                Ok(i) => RpcInvocation {
                                    invocation: Some(i),
                                    reply: m.reply.clone(),
                                },
                                Err(_e) => RpcInvocation {
                                    invocation: None,
                                    reply: None,
                                },
                            }
                        }))
                    }
                }),
        )
    }
}

impl Handler<RpcInvocation> for RpcSubscription {
    type Result = ResponseActFuture<Self, ()>;

    fn handle(&mut self, msg: RpcInvocation, _ctx: &mut Self::Context) -> Self::Result {
        let target = self.target.clone().unwrap();
        let nc = self.nc.as_ref().unwrap().clone();
        Box::pin(
            async move {
                if let Some(inv) = msg.invocation {
                    trace!("Handling inbound RPC call from {}", inv.origin.url());
                    let res = target.send(inv).await; // TODO: convert this into a timeout
                    match res {
                        Ok(ir) => {
                            let _ = nc
                                .publish(msg.reply.as_ref().unwrap(), &serialize(&ir).unwrap())
                                .await;
                        }
                        Err(_) => {
                            error!("Failed to forward RPC call to internal bus");
                        }
                    }
                }
            }
            .into_actor(self),
        )
    }
}

// An RPC subscription will also act as a proxy for the underlying target when in local
// dispatch mode, so it needs to be able to handle invocations as well as rpc invocations
impl Handler<Invocation> for RpcSubscription {
    type Result = ResponseActFuture<Self, InvocationResponse>;

    fn handle(&mut self, msg: Invocation, _ctx: &mut Self::Context) -> Self::Result {
        trace!("RPC subscriber proxying invocation to {}", msg.target.url());
        let target = self.target.clone().unwrap();
        Box::pin(
            async move {
                match target.send(msg.clone()).await {
                    Ok(ir) => ir,
                    Err(_e) => InvocationResponse::error(&msg, "Unresponsive target actor"),
                }
            }
            .into_actor(self),
        )
    }
}

pub(crate) fn subject_prefix(ns_prefix: &Option<String>) -> String {
    format!(
        "wasmbus.rpc.{}",
        ns_prefix.as_ref().unwrap_or(&"default".to_string())
    )
}

pub(crate) fn invoke_subject(ns_prefix: &Option<String>, entity: &WasccEntity) -> String {
    let prefix = subject_prefix(ns_prefix);
    match entity {
        WasccEntity::Actor(s) => format!("{}.{}", prefix, s),
        WasccEntity::Capability { id, link, .. } => format!("{}.{}.{}", prefix, id, link),
    }
}

pub(crate) fn links_subject(ns_prefix: &Option<String>) -> String {
    let prefix = subject_prefix(ns_prefix);
    format!("wasmbus.{}.links", prefix)
}

pub(crate) fn claims_subject(ns_prefix: &Option<String>) -> String {
    let prefix = subject_prefix(ns_prefix);
    format!("wasmbus.{}.claims", prefix)
}
