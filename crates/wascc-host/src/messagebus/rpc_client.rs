use crate::generated::core::{deserialize, serialize};
use crate::messagebus::rpc_subscription::{claims_subject, links_subject};
use crate::messagebus::{AdvertiseBinding, AdvertiseClaims, MessageBus, PutClaims, PutLink};
use crate::Result;
use crate::{Invocation, InvocationResponse, WasccEntity};
use actix::prelude::*;
use futures::StreamExt;
use serde::{Deserialize, Serialize};
use std::collections::HashMap;
use std::sync::Arc;

#[derive(Message)]
#[rtype(result = "()")]
pub(crate) struct Initialize {
    pub nc: Arc<nats::asynk::Connection>,
    pub ns_prefix: Option<String>,
    pub bus: Addr<MessageBus>,
}

#[derive(Default)]
pub(crate) struct RpcClient {
    pub nc: Option<Arc<nats::asynk::Connection>>,
    pub ns_prefix: Option<String>,
    pub bus: Option<Addr<MessageBus>>,
}

#[derive(Message)]
#[rtype(result = "()")]
struct ClaimsInbound {
    claims: Option<wascap::jwt::Claims<wascap::jwt::Actor>>,
}

#[derive(Message)]
#[rtype(result = "()")]
struct LinkInbound {
    link: Option<LinkDefinition>,
}

#[derive(Debug, Serialize, Deserialize, Clone)]
struct LinkDefinition {
    actor: String,
    contract_id: String,
    link_name: String,
    provider_id: String,
    values: HashMap<String, String>,
}

impl Actor for RpcClient {
    type Context = Context<Self>;
}

impl Handler<Initialize> for RpcClient {
    type Result = ResponseActFuture<Self, ()>;

    fn handle(&mut self, msg: Initialize, _ctx: &mut Self::Context) -> Self::Result {
        self.nc = Some(msg.nc);
        self.ns_prefix = msg.ns_prefix;
        self.bus = Some(msg.bus);

        let nc = self.nc.clone().unwrap();
        let prefix = self.ns_prefix.clone();
        Box::pin(
            async move {
                let claims_sub = nc.subscribe(&claims_subject(&prefix)).await;
                let links_sub = nc.subscribe(&links_subject(&prefix)).await;
                (claims_sub, links_sub)
            }
            .into_actor(self)
            .map(|(claims, links), act, ctx| {
                // Set up subscriber for claims advertisements
                if let Ok(c) = claims {
                    ctx.add_message_stream(c.map(|m| {
                        let claims =
                            deserialize::<wascap::jwt::Claims<wascap::jwt::Actor>>(&m.data);
                        match claims {
                            Ok(c) => ClaimsInbound { claims: Some(c) },
                            Err(_) => ClaimsInbound { claims: None },
                        }
                    }));
                }
                // Set up subscriber for links advertisements
                if let Ok(l) = links {
                    ctx.add_message_stream(l.map(|m| {
                        let link = deserialize::<LinkDefinition>(&m.data);
                        match link {
                            Ok(l) => LinkInbound { link: Some(l) },
                            Err(_) => LinkInbound { link: None },
                        }
                    }))
                }
            }),
        )
    }
}

// Perform an RPC call (subject request w/timeout) on the rpc bus
impl Handler<Invocation> for RpcClient {
    type Result = InvocationResponse;

    fn handle(&mut self, msg: Invocation, _ctx: &mut Self::Context) -> Self::Result {
        // TODO: implement

        InvocationResponse::error(&msg, "not implemented")
    }
}

impl Handler<ClaimsInbound> for RpcClient {
    type Result = ResponseActFuture<Self, ()>;

    fn handle(&mut self, msg: ClaimsInbound, _ctx: &mut Self::Context) -> Self::Result {
        let target = self.bus.clone().unwrap();
        if msg.claims.is_some() {
            Box::pin(
                async move {
                    let _ = target
                        .send(PutClaims {
                            claims: msg.claims.unwrap(),
                        })
                        .await;
                }
                .into_actor(self),
            )
        } else {
            Box::pin(async move {}.into_actor(self))
        }
    }
}

impl Handler<LinkInbound> for RpcClient {
    type Result = ResponseActFuture<Self, ()>;

    fn handle(&mut self, msg: LinkInbound, _ctx: &mut Self::Context) -> Self::Result {
        let target = self.bus.clone().unwrap();
        if let Some(link) = msg.link {
            Box::pin(
                async move {
                    let _ = target
                        .send(PutLink {
                            binding_name: link.link_name,
                            contract_id: link.contract_id,
                            provider_id: link.provider_id,
                            actor: link.actor,
                            values: link.values,
                        })
                        .await;
                }
                .into_actor(self),
            )
        } else {
            Box::pin(async move {}.into_actor(self))
        }
    }
}
// Publish a link to the RPC bus
impl Handler<AdvertiseBinding> for RpcClient {
    type Result = Result<()>;

    fn handle(&mut self, msg: AdvertiseBinding, _ctx: &mut Self::Context) -> Self::Result {
        Ok(())
    }
}

impl Handler<AdvertiseClaims> for RpcClient {
    type Result = Result<()>;

    fn handle(&mut self, msg: AdvertiseClaims, _ctx: &mut Self::Context) -> Self::Result {
        Ok(())
    }
}
