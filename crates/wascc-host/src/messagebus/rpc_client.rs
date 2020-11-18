use crate::generated::core::{deserialize, serialize};
use crate::hlreg::HostLocalSystemService;
use crate::host_controller::{CheckLink, HostController};
use crate::messagebus::rpc_subscription::{claims_subject, invoke_subject, links_subject};
use crate::messagebus::{AdvertiseBinding, AdvertiseClaims, MessageBus, PutClaims, PutLink};
use crate::Result;
use crate::{Invocation, InvocationResponse, WasccEntity};
use actix::prelude::*;
use futures::StreamExt;
use serde::{Deserialize, Serialize};
use std::collections::HashMap;
use std::sync::Arc;
use std::time::Duration;

#[derive(Message)]
#[rtype(result = "()")]
pub(crate) struct Initialize {
    pub nc: Arc<nats::asynk::Connection>,
    pub ns_prefix: Option<String>,
    pub bus: Addr<MessageBus>,
    pub rpc_timeout: Duration,
    pub host_id: String,
}

#[derive(Default)]
pub(crate) struct RpcClient {
    nc: Option<Arc<nats::asynk::Connection>>,
    ns_prefix: Option<String>,
    bus: Option<Addr<MessageBus>>,
    rpc_timeout: Duration,
    host_id: Option<String>,
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
pub(crate) struct LinkDefinition {
    pub actor: String,
    pub contract_id: String,
    pub link_name: String,
    pub provider_id: String,
    pub values: HashMap<String, String>,
}

impl Actor for RpcClient {
    type Context = Context<Self>;
}

impl Handler<Initialize> for RpcClient {
    type Result = ResponseActFuture<Self, ()>;

    fn handle(&mut self, msg: Initialize, _ctx: &mut Self::Context) -> Self::Result {
        info!("Initializing lattice RPC client");
        self.nc = Some(msg.nc);
        self.ns_prefix = msg.ns_prefix;
        self.bus = Some(msg.bus);
        self.rpc_timeout = msg.rpc_timeout;
        self.host_id = Some(msg.host_id);

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
    type Result = ResponseActFuture<Self, InvocationResponse>;

    fn handle(&mut self, msg: Invocation, _ctx: &mut Self::Context) -> Self::Result {
        trace!("Performing lattice RPC call to {}", msg.target.url());
        let client = self.nc.clone().unwrap();
        let subject = invoke_subject(&self.ns_prefix, &msg.target);
        let bytes = serialize(&msg).unwrap();
        let timeout = self.rpc_timeout;

        Box::pin(
            async move {
                match actix_rt::time::timeout(timeout, client.request(&subject, &bytes)).await {
                    Ok(r) => match r {
                        Ok(r) => {
                            let ir: Result<InvocationResponse> = deserialize(&r.data);
                            match ir {
                                Ok(ir) => ir,
                                Err(_) => InvocationResponse::error(
                                    &msg,
                                    "RPC - failed to deserialize invocation response",
                                ),
                            }
                        }
                        Err(e) => InvocationResponse::error(&msg, &format!("RPC error: {}", e)),
                    },
                    Err(_) => InvocationResponse::error(&msg, "RPC call timed out"),
                }
            }
            .into_actor(self),
        )
    }
}

impl Handler<ClaimsInbound> for RpcClient {
    type Result = ResponseActFuture<Self, ()>;

    fn handle(&mut self, msg: ClaimsInbound, _ctx: &mut Self::Context) -> Self::Result {
        trace!("Received notification of actor claims added to lattice");
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
        trace!("Received notification of link definition lattice-wide publication");
        let target = self.bus.clone().unwrap();
        let hc = HostController::from_hostlocal_registry(self.host_id.as_ref().unwrap());
        if let Some(link) = msg.link {
            Box::pin(
                async move {
                    let ld = link.clone();
                    let _ = target
                        .send(PutLink {
                            binding_name: link.link_name,
                            contract_id: link.contract_id,
                            provider_id: link.provider_id,
                            actor: link.actor,
                            values: link.values,
                        })
                        .await;
                    let _ = hc.send(CheckLink { linkdef: ld }).await;
                }
                .into_actor(self),
            )
        } else {
            Box::pin(async move {}.into_actor(self))
        }
    }
}
// Publish a link definition to the RPC bus
impl Handler<AdvertiseBinding> for RpcClient {
    type Result = ResponseActFuture<Self, Result<()>>;

    fn handle(&mut self, msg: AdvertiseBinding, _ctx: &mut Self::Context) -> Self::Result {
        trace!("Publishing link definition on lattice");
        let ld = LinkDefinition {
            actor: msg.actor,
            contract_id: msg.contract_id,
            link_name: msg.binding_name,
            provider_id: msg.provider_id,
            values: msg.values,
        };
        let nc = self.nc.clone().unwrap();
        let subject = links_subject(&self.ns_prefix);
        let bytes = serialize(&ld).unwrap(); // we should never fail or own serialize
        Box::pin(
            async move {
                let r = nc.publish(&subject, &bytes).await;
                let _ = nc.flush();
                match r {
                    Ok(_) => Ok(()),
                    Err(_) => Err("Failed to publish link definition".into()),
                }
            }
            .into_actor(self),
        )
    }
}

impl Handler<AdvertiseClaims> for RpcClient {
    type Result = ResponseActFuture<Self, Result<()>>;

    fn handle(&mut self, msg: AdvertiseClaims, _ctx: &mut Self::Context) -> Self::Result {
        trace!("Publishing actor claims on lattice");
        let nc = self.nc.clone().unwrap();
        let subject = claims_subject(&self.ns_prefix);
        let bytes = serialize(&msg.claims).unwrap(); //should never fail
        Box::pin(
            async move {
                let r = nc.publish(&subject, &bytes).await;
                let _ = nc.flush();
                match r {
                    Ok(_) => Ok(()),
                    Err(_) => Err("Failed to publish claims notification".into()),
                }
            }
            .into_actor(self),
        )
    }
}
