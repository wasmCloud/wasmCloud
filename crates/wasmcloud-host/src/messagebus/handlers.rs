use super::MessageBus;
use crate::capability::{extras::EXTRAS_PUBLIC_KEY, link_cache::LinkKey};
use crate::dispatch::{gen_config_invocation, Invocation, InvocationResponse, WasccEntity};
use crate::hlreg::HostLocalSystemService;
use crate::messagebus::rpc_client::RpcClient;
use crate::messagebus::rpc_subscription::{CreateSubscription, RpcSubscription};
use crate::messagebus::{
    AdvertiseClaims, AdvertiseLink, CanInvoke, ClaimsResponse, EnforceLocalActorLinks,
    EnforceLocalLink, EnforceLocalProviderLinks, EstablishAllLinks, FindLinks, FindLinksResponse,
    GetClaims, Initialize, LinkDefinition, LinksResponse, LookupLink, PutClaims, PutLink,
    QueryActors, QueryAllLinks, QueryProviders, QueryResponse, Subscribe, Unsubscribe,
};
use crate::{auth, Result};
use actix::prelude::*;
use std::sync::Arc;

pub const OP_HEALTH_REQUEST: &str = "HealthRequest";
pub const OP_BIND_ACTOR: &str = "BindActor";

impl Supervised for MessageBus {}

impl SystemService for MessageBus {
    fn service_started(&mut self, ctx: &mut Context<Self>) {
        info!("Message Bus started");

        // TODO: make this value configurable
        ctx.set_mailbox_capacity(1000);
        self.hb(ctx);
    }
}

impl HostLocalSystemService for MessageBus {}

impl Actor for MessageBus {
    type Context = Context<Self>;
}

impl Handler<FindLinks> for MessageBus {
    type Result = FindLinksResponse;

    fn handle(&mut self, msg: FindLinks, _ctx: &mut Context<Self>) -> Self::Result {
        let res = self.link_cache.find_links(&msg.link_name, &msg.provider_id);
        FindLinksResponse { links: res }
    }
}

impl Handler<EnforceLocalActorLinks> for MessageBus {
    type Result = ();

    fn handle(&mut self, msg: EnforceLocalActorLinks, ctx: &mut Context<Self>) -> Self::Result {
        for (key, _values) in self.link_cache.all() {
            if key.actor == msg.actor && self.claims_cache.contains_key(&msg.actor) {
                ctx.notify(EnforceLocalLink {
                    actor: key.actor,
                    contract_id: key.contract_id,
                    link_name: key.link_name,
                })
            }
        }
    }
}

impl Handler<EnforceLocalProviderLinks> for MessageBus {
    type Result = ();

    fn handle(&mut self, msg: EnforceLocalProviderLinks, ctx: &mut Context<Self>) -> Self::Result {
        for (key, values) in self.link_cache.all() {
            if key.link_name == msg.link_name && values.provider_id == msg.provider_id {
                ctx.notify(EnforceLocalLink {
                    actor: key.actor,
                    contract_id: key.contract_id,
                    link_name: key.link_name,
                })
            }
        }
    }
}

impl Handler<EnforceLocalLink> for MessageBus {
    type Result = ResponseActFuture<Self, ()>;

    // If the provider responsible for this link is local, and the actor
    // for this link is known to us, then invoke the link binding
    fn handle(&mut self, msg: EnforceLocalLink, _ctx: &mut Context<Self>) -> Self::Result {
        let claims = self.claims_cache.get(&msg.actor);
        if claims.is_none() {
            return Box::pin(async move {}.into_actor(self)); // do not send link invocation for actors we don't know about
        }
        let key = LinkKey {
            actor: msg.actor.to_string(),
            contract_id: msg.contract_id.to_string(),
            link_name: msg.link_name.to_string(),
        };
        let link = self.link_cache.get(&key);
        if link.is_none() {
            return Box::pin(async move {}.into_actor(self)); // do not invoke if we don't have the link in the link cache
        }
        let link = link.unwrap();
        let target = WasccEntity::Capability {
            id: link.provider_id.to_string(),
            contract_id: msg.contract_id.to_string(),
            link_name: msg.link_name.to_string(),
        };
        if let Some(t) = self.subscribers.get(&target) {
            let t = t.clone();
            let inv = gen_config_invocation(
                self.key.as_ref().unwrap(),
                &msg.actor,
                &msg.contract_id,
                &link.provider_id,
                claims.unwrap().clone(),
                msg.link_name.to_string(),
                link.values,
            );
            Box::pin(
                async move {
                    let _ = t.send(inv).await;
                }
                .into_actor(self),
            )
        } else {
            Box::pin(async move {}.into_actor(self))
        }
    }
}

impl Handler<EstablishAllLinks> for MessageBus {
    type Result = ();

    fn handle(&mut self, _msg: EstablishAllLinks, ctx: &mut Context<Self>) -> Self::Result {
        for (key, _value) in self.link_cache.all() {
            if !self.claims_cache.contains_key(&key.actor) {
                continue; // do not send link invocation for actors we don't know about
            }

            ctx.notify(EnforceLocalLink {
                actor: key.actor.to_string(),
                contract_id: key.contract_id.to_string(),
                link_name: key.link_name.to_string(),
            });
        }
    }
}

impl Handler<QueryActors> for MessageBus {
    type Result = QueryResponse;

    fn handle(&mut self, _msg: QueryActors, _ctx: &mut Context<Self>) -> QueryResponse {
        QueryResponse {
            results: self
                .subscribers
                .keys()
                .filter_map(|k| match k {
                    WasccEntity::Actor(s) => Some(s.to_string()),
                    WasccEntity::Capability { .. } => None,
                })
                .collect(),
        }
    }
}

// Receive a notification of claims
impl Handler<PutClaims> for MessageBus {
    type Result = ();

    fn handle(&mut self, msg: PutClaims, ctx: &mut Context<Self>) {
        let subject = msg.claims.subject.to_string();
        self.claims_cache
            .insert(msg.claims.subject.to_string(), msg.claims);

        ctx.notify(EnforceLocalActorLinks { actor: subject });
    }
}

// Receive a link definition through an advertisement
impl Handler<PutLink> for MessageBus {
    type Result = ();

    fn handle(&mut self, msg: PutLink, ctx: &mut Context<Self>) {
        trace!("Messagebus received link definition notification");
        self.link_cache.add_link(
            &msg.actor,
            &msg.contract_id,
            &msg.link_name,
            &msg.provider_id,
            msg.values.clone(),
        );

        ctx.notify(EnforceLocalLink {
            actor: msg.actor.to_string(),
            contract_id: msg.contract_id.to_string(),
            link_name: msg.link_name.to_string(),
        });
    }
}

impl Handler<CanInvoke> for MessageBus {
    type Result = bool;

    fn handle(&mut self, msg: CanInvoke, _ctx: &mut Context<Self>) -> Self::Result {
        let c = self.claims_cache.get(&msg.actor);
        if c.is_none() {
            return false;
        }
        let c = c.unwrap();
        let target = WasccEntity::Capability {
            id: msg.provider_id,
            contract_id: msg.contract_id.to_string(),
            link_name: msg.link_name,
        };
        let pre_auth = if let Some(ref a) = c.metadata {
            if let Some(ref c) = a.caps {
                c.contains(&msg.contract_id)
            } else {
                false
            }
        } else {
            false
        };
        if !pre_auth {
            return false;
        }
        self.authorizer
            .as_ref()
            .unwrap()
            .can_invoke(c, &target, OP_BIND_ACTOR)
    }
}

impl Handler<QueryAllLinks> for MessageBus {
    type Result = LinksResponse;

    fn handle(&mut self, _msg: QueryAllLinks, _ctx: &mut Context<Self>) -> Self::Result {
        let lds = self
            .link_cache
            .all()
            .iter()
            .map(|(k, v)| LinkDefinition {
                actor_id: k.actor.to_string(),
                provider_id: v.provider_id.to_string(),
                contract_id: k.contract_id.to_string(),
                link_name: k.link_name.to_string(),
                values: v.values.clone(),
            })
            .collect();

        LinksResponse { links: lds }
    }
}

impl Handler<QueryProviders> for MessageBus {
    type Result = QueryResponse;

    fn handle(&mut self, _msg: QueryProviders, _ctx: &mut Context<Self>) -> QueryResponse {
        QueryResponse {
            results: self
                .subscribers
                .keys()
                .filter_map(|k| match k {
                    WasccEntity::Capability { id, .. } => Some(id.to_string()),
                    _ => None,
                })
                .collect(),
        }
    }
}

impl Handler<Initialize> for MessageBus {
    type Result = ResponseActFuture<Self, ()>;

    fn handle(&mut self, msg: Initialize, ctx: &mut Context<Self>) -> Self::Result {
        self.key = Some(msg.key);
        self.authorizer = Some(msg.auth);
        self.nc = msg.nc;
        self.namespace = msg.namespace;
        let ns = self.namespace.clone();
        let timeout = msg.rpc_timeout.clone();
        info!("Messagebus initialized");
        if let Some(nc) = self.nc.clone() {
            let rpc_outbound = RpcClient::default().start();
            self.rpc_outbound = Some(rpc_outbound);
            let target = self.rpc_outbound.clone().unwrap();
            let bus = ctx.address().clone();
            let host_id = self.key.as_ref().unwrap().public_key();
            info!("Messagebus initializing with lattice RPC support");
            Box::pin(
                async move {
                    let _ = target
                        .send(super::rpc_client::Initialize {
                            host_id,
                            nc,
                            ns_prefix: ns,
                            bus,
                            rpc_timeout: timeout,
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

impl Handler<AdvertiseLink> for MessageBus {
    type Result = ResponseActFuture<Self, Result<()>>;

    fn handle(&mut self, msg: AdvertiseLink, ctx: &mut Context<Self>) -> Self::Result {
        trace!("Advertisting link definition");
        self.link_cache.add_link(
            &msg.actor,
            &msg.contract_id,
            &msg.link_name,
            &msg.provider_id,
            msg.values.clone(),
        );

        ctx.notify(EnforceLocalLink {
            actor: msg.actor.to_string(),
            contract_id: msg.contract_id.to_string(),
            link_name: msg.link_name.to_string(),
        });

        let advlink = msg.clone();

        let rpc = self.rpc_outbound.clone();
        Box::pin(
            async move {
                if let Some(ref rpc) = rpc {
                    let _ = rpc.send(advlink).await;
                }
                Ok(())
            }
            .into_actor(self),
        )
    }
}

impl Handler<AdvertiseClaims> for MessageBus {
    type Result = ResponseActFuture<Self, Result<()>>;

    fn handle(&mut self, msg: AdvertiseClaims, ctx: &mut Context<Self>) -> Self::Result {
        trace!("Advertising claims");
        self.claims_cache
            .insert(msg.claims.subject.to_string(), msg.claims.clone());

        ctx.notify(EnforceLocalActorLinks {
            actor: msg.claims.subject.to_string(),
        });

        let rpc = self.rpc_outbound.clone();
        Box::pin(
            async move {
                if let Some(rpc) = rpc {
                    let _ = rpc.send(msg).await;
                }
                Ok(())
            }
            .into_actor(self),
        )
    }
}

impl Handler<Invocation> for MessageBus {
    type Result = ResponseActFuture<Self, InvocationResponse>;

    /// Handle an invocation from any source to any target. If there is a local subscriber
    /// then the invocation will be delivered directly to that subscriber. If the subscriber
    /// is not local, _and_ there is a lattice provider configured, then the bus will attempt
    /// to satisfy that call via RPC over lattice.
    fn handle(&mut self, msg: Invocation, _ctx: &mut Context<Self>) -> Self::Result {
        trace!(
            "{}: Handling invocation from {} to {}",
            self.key.as_ref().unwrap().public_key(),
            msg.origin_url(),
            msg.target_url()
        );
        if let Err(e) = auth::authorize_invocation(
            &msg,
            self.authorizer.as_ref().unwrap().clone(),
            &self.claims_cache,
        ) {
            error!("Authorization failure: {}", e);
            return Box::pin(
                async move {
                    InvocationResponse::error(&msg, &format!("Authorization denied: {}", e))
                }.into_actor(self)
            );
        }
        let subscribers = self.subscribers.clone();
        match subscribers.get(&msg.target) {
            Some(target) => {
                trace!("Invocation taking place within bus");
                Box::pin(
                    target
                        .send(msg.clone())
                        .into_actor(self)
                        .map(move |res, _act, _ctx| {
                            if let Ok(r) = res {
                                r
                            } else {
                                InvocationResponse::error(
                                    &msg,
                                    "Mailbox error attempting to perform invocation",
                                )
                            }
                        }),
                )
            }
            None => {
                if self.rpc_outbound.is_none() {
                    warn!("No local subscribers and no RPC client enabled - invocation lost");
                    Box::pin(
                        async move {
                            InvocationResponse::error(
                            &msg,
                            &"No local bus subscribers found, and no lattice RPC client enabled",
                        )
                        }
                        .into_actor(self),
                    )
                } else {
                    trace!("Deferring invocation to lattice (no local subscribers)");
                    let rpc = self.rpc_outbound.clone().unwrap();
                    Box::pin(
                        async move {
                            let ir = rpc.send(msg.clone()).await;
                            match ir {
                                Ok(ir) => ir,
                                Err(e) => InvocationResponse::error(
                                    &msg,
                                    &format!("Error performing lattice RPC {:?}", e),
                                ),
                            }
                        }
                        .into_actor(self),
                    )
                }
            }
        }
    }
}

impl Handler<LookupLink> for MessageBus {
    type Result = Option<String>;

    fn handle(&mut self, msg: LookupLink, _ctx: &mut Self::Context) -> Self::Result {
        self.link_cache
            .find_provider_id(&msg.actor, &msg.contract_id, &msg.link_name)
    }
}

// register interest for an entity that's "on" the bus. if the bus has a
// nats connection, it will register the interest of an RPC subscription proxy. If there is no
// nats connection, it will register the interest of the actual subscriber.
impl Handler<Subscribe> for MessageBus {
    type Result = ResponseActFuture<Self, ()>;

    fn handle(&mut self, msg: Subscribe, _ctx: &mut Context<Self>) -> Self::Result {
        if self.subscribers.contains_key(&msg.interest) {
            trace!("Skipping bus registration - interested party already registered");
            return Box::pin(async move {}.into_actor(self));
        }

        trace!("Bus registered interest for {}", &msg.interest.url());

        let nc = self.nc.clone();
        let ns = self.namespace.clone();
        Box::pin(
            async move {
                let interest = msg.interest.clone();
                if interest.key() == EXTRAS_PUBLIC_KEY {
                    return (interest, msg.subscriber); // extras are not available over lattice as all hosts have it
                }
                let address = if let Some(ref nc) = nc {
                    let addr = RpcSubscription::default().start();
                    let _ = addr
                        .send(CreateSubscription {
                            entity: msg.interest.clone(),
                            target: msg.subscriber,
                            nc: Arc::new(nc.clone()),
                            namespace: ns,
                        })
                        .await;
                    addr.recipient() // RPC subscriber proxy
                } else {
                    msg.subscriber // Actual subscriber
                };
                (interest, address)
            }
            .into_actor(self)
            .map(|(entity, res), act, _ctx| {
                act.subscribers.insert(entity, res);
            }),
        )
    }
}

impl Handler<Unsubscribe> for MessageBus {
    type Result = ();

    fn handle(&mut self, msg: Unsubscribe, _ctx: &mut Context<Self>) {
        trace!("Bus removing interest for {}", msg.interest.url());
        if let None = self.subscribers.remove(&msg.interest) {
            warn!("Attempted to remove a non-existent subscriber");
        }
    }
}

impl Handler<GetClaims> for MessageBus {
    type Result = ClaimsResponse;

    fn handle(&mut self, _msg: GetClaims, _ctx: &mut Context<Self>) -> Self::Result {
        ClaimsResponse {
            claims: self.claims_cache.clone(),
        }
    }
}
