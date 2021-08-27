use crate::dispatch::OP_HALT;
use crate::generated::core::{deserialize, serialize};
use crate::{Invocation, InvocationResponse, WasmCloudEntity};
use actix::prelude::*;
use futures::StreamExt;
use std::sync::Arc;

#[derive(Message)]
#[rtype(result = "()")]
pub(crate) struct CreateSubscription {
    pub entity: WasmCloudEntity,
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
        let subscription = msg.entity.url();

        Box::pin(
            async move { nc.queue_subscribe(&s, &s).await }
                .into_actor(self)
                .map(move |sub, _act, ctx| match sub {
                    Ok(sub) => ctx.add_message_stream(sub.map(|m| {
                        let i = deserialize::<Invocation>(&m.data);
                        match i {
                            Ok(i) => {
                                trace!("Forwarding RpcInvocation {}", i.target_url());
                                RpcInvocation {
                                    invocation: Some(i),
                                    reply: m.reply.clone(),
                                }
                            }
                            Err(e) => {
                                error!("Error deserializing invocation: {}", e);
                                RpcInvocation {
                                    invocation: None,
                                    reply: None,
                                }
                            }
                        }
                    })),
                    Err(e) => {
                        error!("Could not create subscription for {}, {}", subscription, e);
                    }
                }),
        )
    }
}

impl Handler<RpcInvocation> for RpcSubscription {
    type Result = ResponseActFuture<Self, ()>;

    fn handle(&mut self, msg: RpcInvocation, _ctx: &mut Self::Context) -> Self::Result {
        if self.target.is_none() {
            return Box::pin(async move {}.into_actor(self));
        }
        let target = self.target.clone().unwrap();
        let nc = self.nc.as_ref().unwrap().clone();
        Box::pin(
            async move {
                if let Some(inv) = msg.invocation {
                    trace!(
                        "Handling inbound RPC call from {} to {}",
                        inv.origin.url(),
                        inv.target.url()
                    );
                    let res = target.send(inv).await; // TODO: convert this into a timeout
                    match res {
                        Ok(ir) => {
                            let _ = nc
                                .publish(msg.reply.as_ref().unwrap(), &serialize(&ir).unwrap())
                                .await;
                        }
                        Err(_) => {
                            error!("Failed to forward RPC call to internal target");
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

    fn handle(&mut self, msg: Invocation, ctx: &mut Self::Context) -> Self::Result {
        if msg.origin == msg.target && msg.operation == OP_HALT {
            info!("RPC subscription proxy halting, forwarding halt instruction to internal target");
            ctx.stop();
            if let Some(ref target) = self.target {
                let _ = target.do_send(msg.clone());
            }
            self.target = None;
            return Box::pin(
                async move { InvocationResponse::success(&msg, vec![]) }.into_actor(self),
            );
        }
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

pub(crate) fn invoke_subject(ns_prefix: &Option<String>, entity: &WasmCloudEntity) -> String {
    let prefix = subject_prefix(ns_prefix);
    match entity {
        WasmCloudEntity::Actor(s) => format!("{}.{}", prefix, s),
        WasmCloudEntity::Capability { id, link_name, .. } => {
            format!("{}.{}.{}", prefix, id, link_name)
        }
    }
}

pub(crate) fn remove_links_subject(ns_prefix: &Option<String>) -> String {
    let prefix = subject_prefix(ns_prefix);
    format!("{}.remlinks", prefix)
}

pub(crate) fn links_subject(ns_prefix: &Option<String>) -> String {
    let prefix = subject_prefix(ns_prefix);
    format!("{}.links", prefix)
}

pub(crate) fn claims_subject(ns_prefix: &Option<String>) -> String {
    let prefix = subject_prefix(ns_prefix);
    format!("{}.claims", prefix)
}
